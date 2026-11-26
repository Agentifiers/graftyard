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
    Plan, Planned, PlannedMerge, Target, merge_command, merge_request, plan, query, refused, refused_merge, render,
    render_merge, route,
};
use graftyard_api::rest::{
    commit_not_found, get_check_run, get_commit, get_pull, list_check_run_annotations, list_check_runs_for_ref,
    list_commit_statuses, list_issue_comments, list_review_comments, list_reviews,
};
use graftyard_api::view::Repo;
use graftyard_api::wire::Page;
use graftyard_api::wire::{Host, NodeId, NodeType};
use graftyard_fold::store::META;
use graftyard_fold::{MemStore, fold_digest, meta};
use graftyard_git::issue_ops::{IssueErr, IssueOp, IssueOut};
use graftyard_git::issues::{
    AnnotationLevel, AppId, CheckAnnotation, CheckConclusion, CheckOutput, CheckRunId, CheckRunUpdate, CheckStatus,
    CheckSuiteId, CommentTarget, DiffSide, Grouping, IssueNumber, LabelName, LineAnchor, MergeMethod, Protection,
    RepoSettings, RequiredChecks, ReviewState, StatusId, StatusState, StatusUpdate, Url, Visibility,
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

/// The merge inputs of a planned `mergePullRequest`.
fn merge_of(p: &Planned) -> PlannedMerge {
    let Plan::Merge(m) = p.plan else { panic!("not a merge plan: {:?}", p.plan) };
    m
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
    let m = merge_of(&p);
    assert_eq!(m, PlannedMerge { n: IssueNumber(2), method: MergeMethod::Merge, expected_head: None });
    // A method the repository does not allow is refused before any commit.
    let mut rebase = vars.clone().unwrap();
    rebase["input"]["mergeMethod"] = json!("REBASE");
    let rq = Request::parse(&d.query, Some(rebase), None).unwrap();
    assert_eq!(plan(&rq, &r).unwrap_err()["errors"][0]["type"], "UNPROCESSABLE");
    // The seeded rule requires `ci`, which passed, and conversation resolution, with none open: the merge commits.
    let mr = merge_request(&p.field, &m, &r).unwrap();
    assert_eq!((mr.base_old, mr.head), (oid(1), oid(2)));
    drop(r);
    let out = c.commit_merge(merge_command(&m, cache_plan(&mr), who(WRITER))).unwrap();
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
    let e = merge_request(&p.field, &m, &r).unwrap_err();
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
    let m = merge_of(&p);
    let mr = merge_request(&p.field, &m, &r).unwrap();
    drop(r);
    // The base moved after the plan.
    c.push("refs/heads/main", oid(9));
    let e = c.commit_merge(merge_command(&m, cache_plan(&mr), who(WRITER))).unwrap_err();
    let reply = refused_merge(&p, &e);
    assert_eq!(reply["errors"][0]["message"], "Base branch was modified. Review and try the merge again.");
    assert_eq!(reply["errors"][0]["type"], "UNPROCESSABLE");
    // The required check re-run as a new run, queued: refused with the protection text.
    c.commit(WRITER, run(14, 2, "ci", None)).unwrap();
    let r = c.repo(0, &h, &dir);
    let mr = merge_request(&p.field, &m, &r).unwrap();
    drop(r);
    let e = c.commit_merge(merge_command(&m, cache_plan(&mr), who(WRITER))).unwrap_err();
    assert_eq!(refused_merge(&p, &e)["errors"][0]["message"], "Required status check \"ci\" is in progress.");
    // A reader may not merge.
    let r = c.repo(0, &h, &dir);
    let mr = merge_request(&p.field, &m, &r).unwrap();
    drop(r);
    let e = c.commit_merge(merge_command(&m, cache_plan(&mr), who(READER))).unwrap_err();
    assert_eq!(refused_merge(&p, &e)["errors"][0]["type"], "FORBIDDEN");
}

/// A merge plans from `TierRows::merge_rows` alone exactly as from both whole tiers: the same `Planned`, the same
/// `MergeRequest`, and the same refusals (a closed pull request, a method the repository does not allow), so the edge
/// reads only those rows on a fresh leader. The pull request's id names only its own rows. REST's
/// `PUT /pulls/{n}/merge` names it by number (`MergeOf::Number`), and plans from those rows as from both tiers too.
#[test]
fn a_merge_plans_from_its_rows_as_from_both_tiers() {
    use graftyard_api::hosted::{HostedDir, MergeOf, TierRows};
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
    // An outside collaborator (104), who reads the private repository through its row alone.
    c.collaborator(104, graftyard_identity::RepoPermission::Read);
    let h = host();
    let dir = Dir::new(WRITER);
    let hosted = HostedDir { home: Some(dir.home()), viewer: Some(AccountId(DbId(WRITER))), ..Default::default() };
    let d = doc("PullRequestMerge-1");
    let plan_on = |core: &MemStore, meta: &MemStore, vars: &Option<Map<String, Value>>| {
        let r = Repo { host: &h, at: dir.home(), core, meta, dir: &hosted };
        let req = Request::parse(&d.query, vars.clone(), None).unwrap();
        plan(&req, &r).and_then(|p| merge_request(&p.field, &merge_of(&p), &r).map(|m| (p, m)))
    };
    let m = &c.members[0];
    let (vars, id) = {
        let r = c.repo(0, &h, &dir);
        (with_ids(&d, &r), r.item(IssueNumber(2)).unwrap().row.id)
    };
    let rows = TierRows::merge_rows(&m.core, Some(&m.meta), MergeOf::Id(id));
    let crossed: TierRows = serde_json::from_value(serde_json::to_value(&rows).unwrap()).unwrap();
    let (core, meta) = (TierRows::store(&crossed.core), TierRows::store(crossed.meta.as_ref().unwrap()));
    let small = plan_on(&core, &meta, &vars).unwrap();
    assert_eq!(small, plan_on(&m.core, &m.meta, &vars).unwrap());
    assert_eq!((small.1.n, small.1.base_old, small.1.head), (IssueNumber(2), oid(1), oid(2)));
    assert!(crossed.core.len() + crossed.meta.as_ref().unwrap().len() <= 7, "{crossed:?}");
    // The edge's read gate decides on these rows, and on the access rows alone, as on both tiers: `settings` and the
    // collaborator row ride along.
    let (outsider, gate) = (who(103), |core: &MemStore, p: &graftyard_identity::Principal| {
        let access = graftyard_git::issue_ops::Access { repo: REPO, owner: AccountId(DbId(OWNER)), core };
        graftyard_identity::authorize_read(Some(p), None, &[], &access)
    });
    let access = TierRows::store(&TierRows::access_rows(&m.core));
    for (p, want) in [(who(104), graftyard_identity::Decision::Allow), (outsider, graftyard_identity::Decision::Hidden)]
    {
        assert_eq!(gate(&core, &p), want);
        assert_eq!(gate(&access, &p), want);
        assert_eq!(gate(&m.core, &p), want);
    }
    let mut rebase = vars.clone().unwrap();
    rebase["input"]["mergeMethod"] = json!("REBASE");
    assert_eq!(plan_on(&core, &meta, &Some(rebase.clone())), plan_on(&m.core, &m.meta, &Some(rebase)));
    // REST by number: #2's rows plan as both tiers do, and #3's rows are not #2's.
    let rest_on = |core: &MemStore, meta: &MemStore, n: &str| {
        let r = Repo { host: &h, at: dir.home(), core, meta, dir: &hosted };
        graftyard_api::rest::plan_rest_merge(&r, n, &json!({ "merge_method": "squash" })).map_err(|e| e.status)
    };
    let by_n =
        |m: &graftyard_api::hosted::TierRows| (TierRows::store(&m.core), TierRows::store(m.meta.as_ref().unwrap()));
    let (core2, meta2) = by_n(&TierRows::merge_rows(&m.core, Some(&m.meta), MergeOf::Number(IssueNumber(2))));
    let rest = rest_on(&core2, &meta2, "2").unwrap();
    assert_eq!(rest, rest_on(&m.core, &m.meta, "2").unwrap());
    assert_eq!((rest.1.base_old, rest.1.head), (oid(1), oid(2)));
    {
        use graftyard_fold::Read;
        assert!(core2.prefix("pull_head", &[]).len() == 1 && meta2.prefix("issue", &[]).len() == 1, "only #2 rows");
    }
    let (core3, meta3) = by_n(&TierRows::merge_rows(&m.core, Some(&m.meta), MergeOf::Number(IssueNumber(3))));
    assert_eq!(rest_on(&core3, &meta3, "2"), Err(404), "#3's rows hold no #2");
    assert_eq!(rest_on(&core3, &meta3, "3").map(|r| r.1.head), rest_on(&m.core, &m.meta, "3").map(|r| r.1.head));
    // Once #2 merged, its rows refuse a second merge as both tiers do.
    c.commit_merge(merge_command(&merge_of(&small.0), cache_plan(&small.1), who(WRITER))).unwrap();
    let m = &c.members[0];
    let rows = TierRows::merge_rows(&m.core, Some(&m.meta), MergeOf::Id(id));
    let (core, meta) = (TierRows::store(&rows.core), TierRows::store(rows.meta.as_ref().unwrap()));
    let refused = plan_on(&core, &meta, &vars).unwrap_err();
    assert_eq!(refused, plan_on(&m.core, &m.meta, &vars).unwrap_err());
    assert_eq!(refused["errors"][0]["message"], "Pull request is not open");
    let (core2, meta2) = by_n(&TierRows::merge_rows(&m.core, Some(&m.meta), MergeOf::Number(IssueNumber(2))));
    assert_eq!(rest_on(&core2, &meta2, "2").err(), rest_on(&m.core, &m.meta, "2").err());
    assert!(rest_on(&core2, &meta2, "2").is_err(), "a merged pull request is refused by number too");
}

/// A fork's network reaches the edge with the access rows it already reads; a root adds no row and no bytes.
#[test]
fn access_rows_include_a_forks_network_row_but_no_root_row() {
    use graftyard_api::hosted::TierRows;
    use graftyard_git::repo::{RepoEvent, RepoFold};
    use graftyard_raft::{EntryBody, Fold, Format, LogEntry, PREV_ZERO, Term};
    use graftyard_types::{Index, NetworkId, RepoId};

    let repo = RepoId(DbId(7));
    let owner = AccountId(DbId(1));
    let network = NetworkId::of_root(RepoId(DbId(3)));
    let fork_of = Some((RepoId(DbId(3)), Index(9)));
    let mut fork = RepoFold::new(repo, owner);
    fork.apply_entry(&LogEntry {
        term: Term(2),
        index: Index(1),
        prev: PREV_ZERO,
        format: Format { family: 0x20, version: 31 },
        tiers: TierSet::CORE,
        at: Timestamp(1_000),
        requests: Default::default(),
        extend: None,
        body: EntryBody::Events(vec![RepoEvent::Genesis {
            owner,
            network,
            name: graftyard_identity::RepoName::parse("fork").unwrap(),
            fork_of,
        }]),
    });
    let rows = TierRows::access_rows(&fork.core);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "fork");
    assert!(rows[0].1.is_empty());
    assert_eq!(
        graftyard_git::tiers::fork(&TierRows::store(&rows)),
        Some(graftyard_git::tiers::ForkRow { network, fork_of })
    );

    let root = RepoFold::new(repo, owner);
    assert!(TierRows::access_rows(&root.core).is_empty());
}

/// An agent push reads its policy and every task, and no settings, open-task index, ref, or issue rows. The narrow
/// store answers the policy and session lookups exactly as the full tiers do.
#[test]
fn an_agent_push_reads_only_its_policy_and_task_rows() {
    use graftyard_api::hosted::TierRows;
    use graftyard_git::agent::AgentPolicy;
    use graftyard_git::repo::RepoEvent;
    use graftyard_git::task::{AgentTask, TaskActor, TaskApiState, TaskKind, task_key};
    use graftyard_identity::{InstallationId, SessionId, UserId};
    use graftyard_types::Index;

    let mut c = seeded();
    let policy = AgentPolicy { extra_paths: vec!["agent-owned/**".into()], ..AgentPolicy::default() };
    c.entry(EntryBody::Events(vec![RepoEvent::AgentPolicy(policy.clone())]), TierSet::CORE);

    let app = AppId(DbId(5));
    let other_app = AppId(DbId(6));
    let task = |name: &str, app| {
        let session = SessionId::External { app, name: name.into() };
        AgentTask {
            session,
            kind: TaskKind::External,
            state: TaskApiState::InProgress,
            prefix: None,
            requester: Some(UserId(AccountId(DbId(WRITER)))),
            app,
            installation: InstallationId(DbId(900 + app.0.0)),
            creator: TaskActor::User(UserId(AccountId(DbId(WRITER)))),
            issue: None,
            snapshot: Index(1),
            prompt: None,
            pr: None,
            summary: format!("task {name}").into(),
        }
    };
    let owned = task("owned", app);
    let other = task("other", other_app);
    let owned_id = owned.session.clone();
    c.entry(
        EntryBody::Events(vec![RepoEvent::TaskCreated(owned), RepoEvent::TaskCreated(other)]),
        TierSet::CORE.union(TierSet::METADATA),
    );

    let m = &c.members[0];
    let rows = TierRows::agent_rows(&m.core);
    let actual: Vec<_> = rows.iter().map(|(table, key, _)| (table.as_str(), key.clone())).collect();
    assert_eq!(
        actual,
        vec![
            ("agent_policy", Vec::new()),
            ("agent_task", task_key(&owned_id)),
            ("agent_task", task_key(&SessionId::External { app: other_app, name: "other".into() })),
        ]
    );

    let narrow = TierRows::store(&rows);
    assert_eq!(graftyard_git::tiers::agent_policy(&narrow), policy);
    assert_eq!(graftyard_git::tiers::agent_policy(&narrow), graftyard_git::tiers::agent_policy(&m.core));
    let full_sessions = graftyard_git::task::app_sessions(&m.core, app);
    assert_eq!(full_sessions, vec![owned_id]);
    assert_eq!(graftyard_git::task::app_sessions(&narrow, app), full_sessions);
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

/// The whole-tier read leaves out the metadata tables no view reads (`UNSERVED`: each agent ref's `PathSet` and each
/// commit's provenance), and every captured query answers from the rows it does read as from the member's own tiers.
#[test]
fn the_tier_read_leaves_out_path_sets_and_provenance_only() {
    use graftyard_api::hosted::{Budget, TierRows};
    use graftyard_fold::{Store, Stored, Val};
    use graftyard_git::tiers::{CORE, METADATA};
    let mut c = seeded();
    let row = |b: &[u8]| Stored { written: graftyard_types::Index(1), cols: vec![Val::Blob(b.into())] };
    let m = &mut c.members[0];
    m.meta.put("path_set", vec![Val::Blob(b"refs/heads/claude/a".as_slice().into())], row(&[7; 2_000]));
    m.meta.put("provenance", vec![Val::Blob([1; 20].as_slice().into())], row(&[8; 64]));
    let (core, ok1) = TierRows::dump(&c.members[0].core, &CORE, &mut Budget::rows(100_000));
    let (meta, ok2) = TierRows::dump(&c.members[0].meta, &METADATA, &mut Budget::rows(100_000));
    assert!(ok1 && ok2);
    let tables: std::collections::BTreeSet<&str> = meta.iter().map(|(t, _, _)| t.as_str()).collect();
    assert!(!tables.contains("path_set") && !tables.contains("provenance"), "{tables:?}");
    assert!(tables.contains("comment") && tables.contains("issue"), "{tables:?}");
    let (core, meta) = (TierRows::store(&core), TierRows::store(&meta));
    let (h, dir) = (host(), Dir::new(WRITER));
    let full = c.repo(0, &h, &dir);
    let read = graftyard_api::view::Repo { host: &h, at: dir.home(), core: &core, meta: &meta, dir: &dir };
    for f in every_doc() {
        let d = doc(&f);
        if d.query.trim_start().starts_with("mutation") {
            continue;
        }
        let vars = with_ids(&d, &full);
        assert_eq!(ask(&read, &d, vars.clone()), ask(&full, &d, vars), "{f}");
    }
}

/// The hosted path: the edge reads the tiers as `TierRows` JSON and plans, the command crosses as JSON with its
/// `Render`, and the member renders from `&Acked` over its own tiers. The reply equals the in-process render.
#[test]
fn a_hosted_mutation_renders_on_the_member_as_in_process() {
    use graftyard_api::hosted::{Budget, HostedDir, Render, TierRows, What, render_ok};
    use graftyard_git::tiers::{CORE, METADATA};
    let mut c = seeded();
    let h = host();
    let dir = Dir::new(WRITER);
    let rows = |c: &Chain| {
        let (mut budget, m) = (Budget::rows(100_000), &c.members[0]);
        let (core, ok1) = TierRows::dump(&m.core, &CORE, &mut budget);
        let (meta, ok2) = TierRows::dump(&m.meta, &METADATA, &mut budget);
        let t = TierRows { index: 1, core, meta: Some(meta), complete: ok1 && ok2, picks: None };
        serde_json::from_value::<TierRows>(serde_json::to_value(&t).unwrap()).unwrap()
    };
    let t = rows(&c);
    assert!(t.complete);
    // Packed, the member's answer is one JSON string, and it reads back as the same rows; an unpacked answer from a
    // member built before `packed` reads the same.
    let packed = t.packed();
    assert!(packed.is_string());
    assert_eq!(TierRows::from_value(&packed).unwrap(), t);
    assert_eq!(TierRows::from_value(&serde_json::to_value(&t).unwrap()).unwrap(), t);
    let (core, meta) = (TierRows::store(&t.core), TierRows::store(t.meta.as_ref().unwrap()));
    // The rows `Repo` reads are the member's; the written index and the tier's `META` row do not cross.
    let again = |s: &MemStore, schema| TierRows::dump(s, schema, &mut Budget::rows(100_000)).0;
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

/// A29 and A30 on the edge's read: a single-PR read that renders `mergeable` or `mergeStateStatus` with no current test
/// merge asks for that PR's merge at its (base tip, head); a list that renders it asks for nothing to compute, only a
/// count; a read that does not select it, or a PR whose current merge is recorded, asks nothing.
#[test]
fn a_read_asks_for_the_merge_its_mergeable_lacks_and_a_list_only_counts() {
    use graftyard_api::graphql::query_asks;
    use graftyard_git::issues::MergeabilityResult;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let run = |c: &Chain, q: &str| {
        let r = c.repo(0, &h, &dir);
        query_asks(&Request::parse(q, None, None).unwrap(), Some(&r)).1
    };
    let single = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { mergeable } } }"#;
    let state = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { mergeStateStatus } } }"#;
    let title = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { title } } }"#;
    let list = r#"query { repository(owner: "o", name: "r") { pullRequests(first: 10) { nodes { mergeable } } } }"#;
    let a = run(&c, single);
    assert_eq!(a.single.into_iter().collect::<Vec<_>>(), vec![(IssueNumber(2), (oid(1), oid(2)))], "base tip, head");
    assert!(a.listed.is_empty());
    assert_eq!(run(&c, state).single.len(), 1, "mergeStateStatus reads the merge too");
    assert_eq!(run(&c, title), Default::default(), "no field that reads the merge, no ask");
    let l = run(&c, list);
    assert!(l.single.is_empty(), "a list never asks for a compute");
    assert_eq!(l.listed.into_iter().collect::<Vec<_>>(), vec![IssueNumber(2)]);
    // Outside `query_asks` nothing is recorded, and the next `query_asks` starts empty.
    let r = c.repo(0, &h, &dir);
    let _ = graftyard_api::graphql::query(&Request::parse(single, None, None).unwrap(), Some(&r));
    assert_eq!(run(&c, title), Default::default());
    // Its current merge recorded: nothing to ask.
    let ev = graftyard_git::repo::RepoEvent::TestMerged {
        n: IssueNumber(2),
        base: oid(1),
        head: oid(2),
        result: MergeabilityResult::Clean { merge: oid(9) },
        pack: None,
    };
    c.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    assert_eq!(run(&c, single), Default::default());
    assert_eq!(run(&c, list), Default::default());
    // The base moves: the recorded pair is stale, and the ask names the new tip.
    c.push("refs/heads/main", oid(5));
    assert_eq!(run(&c, single).single.get(&IssueNumber(2)), Some(&(oid(5), oid(2))));
}

/// merge-budget-3 item 1: REST `GET pulls/{n}` asks for the same test merge GraphQL's single-PR read asks for, at the
/// same (base tip, head), and only when that read would: a PR with its current merge recorded, an issue, an unknown
/// number, or a PR from another repository asks nothing. Until the merge is recorded the reply is GHES 3.22's
/// `mergeable` null and `mergeable_state` "unknown"; once it is, `mergeable` is its result and `mergeable_state` the
/// lower case of GraphQL's `mergeStateStatus` (blocked by an unmet required check), and a base that moves past the
/// record makes it null again.
#[test]
fn a_rest_pull_read_asks_for_the_merge_graphql_asks_for_and_keeps_mergeable_null() {
    use graftyard_api::graphql::query_asks;
    use graftyard_api::rest::{get_pull, get_pull_asks};
    use graftyard_git::issues::MergeabilityResult;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let gql = |c: &Chain| {
        let r = c.repo(0, &h, &dir);
        let q = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { mergeable } } }"#;
        query_asks(&Request::parse(q, None, None).unwrap(), Some(&r)).1
    };
    let rest = |c: &Chain, n: &str| {
        let r = c.repo(0, &h, &dir);
        let (reply, asks) = get_pull_asks(&r, n);
        assert_eq!(reply.value(), get_pull(&r, n).value(), "the same bytes as before");
        (reply, asks)
    };
    let (reply, asks) = rest(&c, "2");
    assert_eq!(asks, gql(&c), "the merge GraphQL asks for");
    assert_eq!(asks.single.into_iter().collect::<Vec<_>>(), vec![(IssueNumber(2), (oid(1), oid(2)))], "base tip, head");
    assert_eq!((&reply.value()["mergeable"], &reply.value()["mergeable_state"]), (&json!(null), &json!("unknown")));
    assert_eq!(rest(&c, "1").1, Default::default(), "an issue asks nothing");
    assert_eq!(rest(&c, "99").1, Default::default());
    // The base moves: the ask names the new tip.
    c.push("refs/heads/main", oid(5));
    assert_eq!(rest(&c, "2").1.single.get(&IssueNumber(2)), Some(&(oid(5), oid(2))));
    let state = |c: &Chain| {
        let r = c.repo(0, &h, &dir);
        let q = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) { mergeStateStatus } } }"#;
        let v = graftyard_api::graphql::query_asks(&Request::parse(q, None, None).unwrap(), Some(&r)).0;
        v["data"]["repository"]["pullRequest"]["mergeStateStatus"].as_str().unwrap().to_ascii_lowercase()
    };
    let merged = |c: &mut Chain, base: u8, result: MergeabilityResult| {
        let ev = graftyard_git::repo::RepoEvent::TestMerged {
            n: IssueNumber(2),
            base: oid(base),
            head: oid(2),
            result,
            pack: None,
        };
        c.entry(EntryBody::Events(vec![ev]), TierSet::CORE);
    };
    // Its current merge recorded: nothing to ask, and REST answers the result as GraphQL does.
    merged(&mut c, 5, MergeabilityResult::Clean { merge: oid(9) });
    let (reply, asks) = rest(&c, "2");
    assert_eq!(asks, Default::default());
    assert_eq!(asks, gql(&c));
    assert_eq!((&reply.value()["mergeable"], &reply.value()["mergeable_state"]), (&json!(true), &json!("clean")));
    assert_eq!(reply.value()["mergeable_state"], json!(state(&c)));
    // The required `ci` re-run, in progress: still mergeable, but blocked, as GraphQL says.
    c.commit(WRITER, run(30, 2, "ci", None)).unwrap();
    let reply = rest(&c, "2").0;
    assert_eq!((&reply.value()["mergeable"], &reply.value()["mergeable_state"]), (&json!(true), &json!("blocked")));
    assert_eq!(reply.value()["mergeable_state"], json!(state(&c)));
    // A conflict at a newer base tip: false and dirty.
    c.push("refs/heads/main", oid(6));
    merged(&mut c, 6, MergeabilityResult::Conflict { paths: Default::default() });
    let reply = rest(&c, "2").0;
    assert_eq!((&reply.value()["mergeable"], &reply.value()["mergeable_state"]), (&json!(false), &json!("dirty")));
    assert_eq!(reply.value()["mergeable_state"], json!(state(&c)));
    // The base moves past the record: null again, and the ask names the new tip.
    c.push("refs/heads/main", oid(7));
    let (reply, asks) = rest(&c, "2");
    assert_eq!((&reply.value()["mergeable"], &reply.value()["mergeable_state"]), (&json!(null), &json!("unknown")));
    assert_eq!(asks.single.get(&IssueNumber(2)), Some(&(oid(7), oid(2))));
}

/// BF-77's token half (`decisions/DECISIONS.md`, "`app-bot`"): `POST /repos/{o}/{r}/issues` plans `createIssue`'s
/// `OpenIssue` from GHES's body and renders the pinned `issue` with `Location` from `&Acked`; and a status an App sets
/// names its bot account, which renders as GitHub's `github-actions[bot]`.
#[test]
fn rest_issue_create_and_a_bot_status_render_ghes_shapes() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_err, render_ok};
    use graftyard_api::rest::plan_create_issue;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let body = json!({ "title": "from rest", "body": "b", "labels": ["BUG"], "assignees": ["alice"] });
    let op = plan_create_issue(&c.repo(0, &h, &dir), &body).unwrap();
    let IssueOp::OpenIssue { labels, assignees, .. } = &op else { panic!("{op:?}") };
    assert_eq!((labels.len(), assignees.as_slice()), (1, &[AccountId(DbId(WRITER))][..]), "by name and login");
    // A numeric title is GHES's `oneOf`; what `createIssue` refuses, or a name the repository lacks, is 422.
    let IssueOp::OpenIssue { title, .. } = plan_create_issue(&c.repo(0, &h, &dir), &json!({ "title": 7 })).unwrap()
    else {
        panic!()
    };
    assert_eq!(&*title, "7");
    for bad in [
        json!({}),
        json!({ "title": "t", "milestone": 1 }),
        json!({ "title": "t", "labels": ["nope"] }),
        json!({ "title": "t", "assignee": "nobody" }),
    ] {
        assert_eq!(plan_create_issue(&c.repo(0, &h, &dir), &bad).unwrap_err().status, 422, "{bad}");
    }
    let out = c.commit(WRITER, op).unwrap();
    let mut logins: std::collections::BTreeMap<i64, String> =
        [(OWNER, "o".to_owned()), (WRITER, "alice".to_owned()), (READER, "bob".to_owned())].into();
    let app = graftyard_identity::stateless::ACTIONS_APP;
    logins.insert(app.bot().0.0, "github-actions[bot]".into());
    let hosted = HostedDir { home: Some(dir.home()), logins, ..Default::default() };
    let render = |what: What| Render { host: h.name().to_owned(), isolation: false, dir: hosted.clone(), what };
    let m = &c.members[0];
    let reply = render_ok(&render(What::IssueCreated), &ack(out), &m.core, &m.meta).reply();
    assert_eq!(reply.status, 201);
    let v = reply.value();
    assert_eq!((v["number"].clone(), v["title"].clone()), (json!(4), json!("from rest")));
    assert_eq!((v["user"]["login"].clone(), v["assignees"][0]["login"].clone()), (json!("alice"), json!("alice")));
    assert_eq!(v["labels"][0]["name"], "bug");
    assert_eq!(
        reply.headers.iter().find(|(k, _)| *k == "location").map(|(_, l)| l.as_str()),
        Some("https://ghes.example.com/api/v3/repos/o/r/issues/4")
    );
    keep_rest_as("issue-created", "POST", "/repos/{owner}/{repo}/issues", &reply);
    // An installation token without `issues: write`: GitHub's 403.
    let forbidden = render_err(&render(What::IssueCreated), &IssueErr::Forbidden).reply();
    assert_eq!(
        (forbidden.status, forbidden.value()["message"].clone()),
        (403, json!("Resource not accessible by integration"))
    );

    // The Actions App's status, as `decide` commits it for a `GITHUB_TOKEN` with `statuses: write`.
    let st = StatusUpdate {
        id: StatusId(DbId(20)),
        sha: oid(2),
        context: "bf77".into(),
        state: StatusState::Success,
        target_url: None,
        description: None,
        creator: app.bot(),
    };
    c.entry(EntryBody::Events(vec![graftyard_git::repo::RepoEvent::Status(st)]), TierSet::METADATA);
    let m = &c.members[0];
    let sha = oid(2);
    let reply =
        render_ok(&render(What::StatusCreated { sha, id: 20 }), &ack(IssueOut::Id(DbId(20))), &m.core, &m.meta).reply();
    assert_eq!(reply.status, 201);
    let creator = reply.value()["creator"].clone();
    assert_eq!(creator["login"], "github-actions[bot]");
    assert_eq!((creator["id"].clone(), creator["type"].clone()), (json!(2), json!("Bot")));
    assert_eq!(creator["html_url"], "https://ghes.example.com/apps/github-actions");
    assert_eq!(creator["url"], "https://ghes.example.com/api/v3/users/github-actions%5Bbot%5D");
    let node = NodeId::parse(creator["node_id"].as_str().unwrap()).unwrap();
    assert_eq!((node.ty, node.id()), (NodeType::Bot, 2));
    keep_rest_as("status-created-bot", "POST", "/repos/{owner}/{repo}/statuses/{sha}", &reply);
    // A user's status keeps a user's `simple-user`.
    let r = c.repo(0, &h, &dir);
    let user = list_commit_statuses(&r, &sha.hex(), Page::parse("per_page=100")).value().clone();
    let mine = user.as_array().unwrap().iter().find(|s| s["context"] == "jenkins").unwrap();
    assert_eq!((mine["creator"]["login"].clone(), mine["creator"]["type"].clone()), (json!("alice"), json!("User")));
}

/// F137: `POST /repos/{o}/{r}/issues/{n}/comments` plans `addComment`'s `Comment` on an issue or a pull request's
/// conversation and renders the pinned `issue-comment` with `Location` from `&Acked`. A missing item is 404 and a
/// missing `body` 422, before any commit.
#[test]
fn rest_issue_comment_create_renders_the_issue_comment() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_err, render_ok};
    use graftyard_api::rest::plan_create_issue_comment;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let hosted =
        HostedDir { home: Some(dir.home()), logins: [(WRITER, "alice".to_owned())].into(), ..Default::default() };
    let render = |n| Render {
        host: h.name().to_owned(),
        isolation: false,
        dir: hosted.clone(),
        what: What::IssueCommentCreated { n },
    };
    for bad in [json!({}), json!({ "body": 7 })] {
        let e = plan_create_issue_comment(&c.repo(0, &h, &dir), "1", &bad).unwrap_err();
        assert_eq!((e.status, e.value()["errors"][0]["field"].clone()), (422, json!("body")), "{bad}");
    }
    for missing in ["99", "x"] {
        assert_eq!(
            plan_create_issue_comment(&c.repo(0, &h, &dir), missing, &json!({ "body": "b" })).unwrap_err().status,
            404
        );
    }
    // Issue 1 has bob's comment; pull request 2 has none. Each new comment is on its own item, after the others.
    for (n, kind, count) in [(1u32, "issues", 2), (2, "pull", 1)] {
        let op =
            plan_create_issue_comment(&c.repo(0, &h, &dir), &n.to_string(), &json!({ "body": "from rest" })).unwrap();
        let out = c.commit(WRITER, op).unwrap();
        let IssueOut::Id(id) = out else { panic!("{out:?}") };
        let m = &c.members[0];
        let reply = render_ok(&render(n), &ack(out), &m.core, &m.meta).reply();
        assert_eq!(reply.status, 201);
        let v = reply.value();
        assert_eq!(
            (v["id"].clone(), v["body"].clone(), v["user"]["login"].clone()),
            (json!(id.0), json!("from rest"), json!("alice"))
        );
        let url = format!("https://ghes.example.com/api/v3/repos/o/r/issues/comments/{}", id.0);
        assert_eq!(
            (v["url"].as_str(), v["issue_url"].clone()),
            (Some(url.as_str()), json!(format!("https://ghes.example.com/api/v3/repos/o/r/issues/{n}")))
        );
        assert_eq!(v["html_url"], format!("https://ghes.example.com/o/r/{kind}/{n}#issuecomment-{}", id.0));
        assert_eq!(reply.headers.iter().find(|(k, _)| *k == "location").map(|(_, l)| l.as_str()), Some(url.as_str()));
        let list = list_issue_comments(&c.repo(0, &h, &dir), &n.to_string(), "").value().clone();
        assert_eq!((list.as_array().unwrap().len(), list[count - 1]["id"].clone()), (count, json!(id.0)), "{n}");
        keep_rest_as(
            &format!("issue-comment-created-{n}"),
            "POST",
            "/repos/{owner}/{repo}/issues/{issue_number}/comments",
            &reply,
        );
    }
    let forbidden = render_err(&render(1), &IssueErr::Forbidden).reply();
    assert_eq!(
        (forbidden.status, forbidden.value()["message"].clone()),
        (403, json!("Resource not accessible by integration"))
    );
    assert_eq!(render_err(&render(1), &IssueErr::Invalid("a body")).reply().status, 422);
}

/// F20's REST assignment: `POST .../assignees` adds and answers 201 with the issue, `DELETE` removes and answers 200,
/// an assignees-only `PATCH` replaces the set, and a login no account claims is dropped, as GHES 3.22 drops it. A
/// `PATCH` that sets any other field is not this route.
#[test]
fn rest_assignees_add_remove_and_replace_render_the_issue() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_ok};
    use graftyard_api::rest::plan_issue_assignees;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let (alice, bob) = (AccountId(DbId(WRITER)), AccountId(DbId(READER)));
    let plan = |c: &Chain, m: &str, body: Value| plan_issue_assignees(&c.repo(0, &h, &dir), m, "1", &body);
    // A reader is dropped by `decide`, as GHES drops an assignee without push access.
    let op = plan(&c, "POST", json!({ "assignees": ["bob"] })).unwrap().unwrap();
    c.commit(WRITER, op).unwrap();
    assert_eq!(c.repo(0, &h, &dir).assignees_of(IssueNumber(1)), [alice]);
    c.collaborator(READER, graftyard_identity::RepoPermission::Write);
    assert!(plan(&c, "PATCH", json!({ "title": "t", "assignees": ["bob"] })).is_none());
    assert_eq!(plan_issue_assignees(&c.repo(0, &h, &dir), "POST", "99", &json!({})).unwrap().unwrap_err().status, 404);
    // Issue 1 opened with alice. `POST` of bob and an unknown login adds bob only.
    let op = plan(&c, "POST", json!({ "assignees": ["bob", "nobody"] })).unwrap().unwrap();
    let IssueOp::Assign { add, remove, .. } = &op else { panic!("{op:?}") };
    assert_eq!((add.as_slice(), remove.as_slice()), (&[bob][..], &[][..]));
    // `PATCH` with only `assignees` replaces: bob added, alice removed.
    let op2 = plan(&c, "PATCH", json!({ "assignees": [{ "login": "bob" }] })).unwrap().unwrap();
    let IssueOp::Assign { add, remove, .. } = &op2 else { panic!("{op2:?}") };
    assert_eq!((add.as_slice(), remove.as_slice()), (&[bob][..], &[alice][..]));
    let out = c.commit(WRITER, op).unwrap();
    let logins: std::collections::BTreeMap<i64, String> =
        [(OWNER, "o".to_owned()), (WRITER, "alice".to_owned()), (READER, "bob".to_owned())].into();
    let hosted = HostedDir { home: Some(dir.home()), logins, ..Default::default() };
    let render = |what: What| Render { host: h.name().to_owned(), isolation: false, dir: hosted.clone(), what };
    let m = &c.members[0];
    let reply = render_ok(&render(What::IssueAssigned { n: 1, created: true }), &ack(out), &m.core, &m.meta).reply();
    assert_eq!(reply.status, 201);
    let mut got: Vec<String> =
        reply.value()["assignees"].as_array().unwrap().iter().map(|a| a["login"].as_str().unwrap().into()).collect();
    got.sort();
    assert_eq!(got, ["alice", "bob"]);
    keep_rest_as("issue-assignees-added", "POST", "/repos/{owner}/{repo}/issues/{issue_number}/assignees", &reply);
    // `DELETE` of alice: 200, bob alone.
    let op = plan(&c, "DELETE", json!({ "assignees": ["alice"] })).unwrap().unwrap();
    let out = c.commit(WRITER, op).unwrap();
    let m = &c.members[0];
    let reply = render_ok(&render(What::IssueAssigned { n: 1, created: false }), &ack(out), &m.core, &m.meta).reply();
    assert_eq!((reply.status, reply.value()["assignees"].as_array().unwrap().len()), (200, 1));
    assert_eq!(reply.value()["assignee"]["login"], "bob");
}

/// F141: an issue `PATCH` whose body names exactly one of `title` and `body` is one title or body `Change`, and its 200
/// is the issue after the entry. Two fields, `state`, or an assignees-only body are not this route (M4-WP4's atomic
/// update and F20's assignment own them), so a two-field `PATCH` never splits into commands that are not atomic.
#[test]
fn rest_issue_patch_of_one_field_is_one_change() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_ok};
    use graftyard_api::rest::plan_issue_edit;
    use graftyard_git::issues::IssueChange;
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let plan = |c: &Chain, n: &str, body: Value| plan_issue_edit(&c.repo(0, &h, &dir), n, &body);
    for multi in [
        json!({ "title": "t", "body": "b" }),
        json!({ "state": "closed" }),
        json!({ "title": "t", "state": "closed" }),
        json!({ "assignees": ["bob"] }),
        json!({}),
        json!("title"),
    ] {
        assert!(plan(&c, "1", multi.clone()).is_none(), "{multi}");
    }
    assert_eq!(plan(&c, "99", json!({ "body": "b" })).unwrap().unwrap_err().status, 404);
    assert_eq!(plan(&c, "1", json!({ "title": ["t"] })).unwrap().unwrap_err().status, 422);
    let IssueOp::Change { change: IssueChange::Title(t), .. } = plan(&c, "1", json!({ "title": 42 })).unwrap().unwrap()
    else {
        panic!("a numeric title is a title change")
    };
    assert_eq!(&*t, "42");
    let IssueOp::Change { change: IssueChange::Body(b), .. } = plan(&c, "1", json!({ "body": null })).unwrap().unwrap()
    else {
        panic!("a null body clears it")
    };
    assert_eq!(&*b, "");
    // The issue's author, a reader, edits its body: committed, and the 200 renders the new body.
    let op = plan(&c, "1", json!({ "body": "steps, again" })).unwrap().unwrap();
    let out = c.commit(READER, op).unwrap();
    let logins: std::collections::BTreeMap<i64, String> =
        [(OWNER, "o".to_owned()), (WRITER, "alice".to_owned()), (READER, "bob".to_owned())].into();
    let hosted = HostedDir { home: Some(dir.home()), logins, ..Default::default() };
    let render = Render {
        host: h.name().to_owned(),
        isolation: false,
        dir: hosted,
        what: What::IssueAssigned { n: 1, created: false },
    };
    let m = &c.members[0];
    let reply = render_ok(&render, &ack(out), &m.core, &m.meta).reply();
    assert_eq!(
        (reply.status, reply.value()["body"].clone(), reply.value()["title"].clone()),
        (200, json!("steps, again"), json!("it breaks"))
    );
    keep_rest_as("issue-patched", "PATCH", "/repos/{owner}/{repo}/issues/{issue_number}", &reply);
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

/// `gh pr edit --body` (AM1's plan blocks): `updatePullRequest` with `body` commits `IssueChange::Body` and renders
/// `UpdatePullRequestPayload.pullRequest` with the new body; a field that is not served, or body and title together,
/// is refused before anything commits.
#[test]
fn update_pull_request_edits_the_body_and_refuses_what_it_does_not_serve() {
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let q = "mutation($input: UpdatePullRequestInput!) { updatePullRequest(input: $input) { pullRequest { number body } } }";
    let (planned, vars) = {
        let r = c.repo(0, &h, &dir);
        let id = NodeId::scoped(NodeType::PullRequest, REPO, r.item(IssueNumber(2)).unwrap().row.id.0).encode();
        let vars = json!({ "input": { "pullRequestId": id, "body": "edited" } }).as_object().cloned();
        let req = Request::parse(q, vars.clone(), None).unwrap();
        assert_eq!(route(&req).unwrap(), Some(Target::Id(REPO.0.0)));
        let p = plan(&req, &r).unwrap();
        let mut both = vars.clone().unwrap();
        both["input"]["title"] = json!("t");
        let req2 = Request::parse(q, Some(both.clone()), None).unwrap();
        assert_eq!(plan(&req2, &r).unwrap_err()["errors"][0]["type"], "UNPROCESSABLE");
        both["input"].as_object_mut().unwrap().remove("title");
        both["input"]["state"] = json!("CLOSED");
        let req3 = Request::parse(q, Some(both), None).unwrap();
        assert_eq!(plan(&req3, &r).unwrap_err()["errors"][0]["type"], "UNPROCESSABLE");
        (p, vars)
    };
    let Plan::Issue(op) = planned.plan.clone() else { panic!() };
    let out = c.commit(WRITER, *op).unwrap();
    let r = c.repo(0, &h, &dir);
    let req = Request::parse(q, vars, None).unwrap();
    let reply = render(&req, &planned, &ack(out), &r);
    assert_eq!(reply["data"]["updatePullRequest"]["pullRequest"]["body"], "edited", "{reply}");
}

/// B0-36's two missing write surfaces, on the seeded chain: `markPullRequestReadyForReview` and
/// `convertPullRequestToDraft` (gh's `pr ready` and `pr ready --undo`), `addLabelsToLabelable` and
/// `removeLabelsFromLabelable` (gh's `issue edit --add-label` / `--remove-label`), and REST
/// `POST`/`PUT`/`DELETE /repos/{o}/{r}/issues/{n}/labels[/{name}]`. Each commits its existing `IssueOp` and renders
/// from `&Acked`; the replies go to `validate.py`.
#[test]
fn ready_for_review_and_issue_labels_commit_and_render_as_ghes_does() {
    use graftyard_api::rest::{issue_labels_ok, plan_issue_labels};
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(WRITER));
    let wip = match c
        .commit(
            WRITER,
            IssueOp::DefineLabel { id: None, name: LabelName("wip".into()), color: [1, 2, 3], description: None },
        )
        .unwrap()
    {
        IssueOut::Id(id) => id,
        o => panic!("{o:?}"),
    };
    c.push("refs/heads/d", oid(5));
    let draft = IssueOp::OpenPr {
        head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name("refs/heads/d") },
        base: name("refs/heads/main"),
        title: "draft".into(),
        body: "".into(),
        draft: true,
        head_sha: oid(5),
        base_sha: oid(1),
    };
    let IssueOut::Number(pr) = c.commit(WRITER, draft).unwrap() else { panic!() };
    let gql = |c: &mut Chain, field: &str, input: Value, select: &str| -> Value {
        let q = format!("mutation($input: {}Input!) {{ {field}(input: $input) {{ clientMutationId {select} }} }}", {
            let mut s = field.to_owned();
            s[..1].make_ascii_uppercase();
            s
        });
        let vars = json!({ "input": input }).as_object().cloned();
        let p = {
            let r = c.repo(0, &h, &dir);
            plan(&Request::parse(&q, vars.clone(), None).unwrap(), &r).unwrap()
        };
        let Plan::Issue(op) = p.plan.clone() else { panic!("{field}") };
        let acked = ack(c.commit(WRITER, *op).unwrap());
        let r = c.repo(0, &h, &dir);
        let reply = render(&Request::parse(&q, vars.clone(), None).unwrap(), &p, &acked, &r);
        assert!(reply.get("errors").is_none(), "{field}: {reply}");
        let d = Doc { name: field.into(), query: q, variables: vars.clone() };
        keep(&format!("b036-{field}"), &d, &vars, &reply);
        reply
    };
    let pr_id = {
        let r = c.repo(0, &h, &dir);
        NodeId::scoped(NodeType::PullRequest, REPO, r.item(pr).unwrap().row.id.0).encode()
    };
    let ready = gql(
        &mut c,
        "markPullRequestReadyForReview",
        json!({ "pullRequestId": pr_id }),
        "pullRequest { isDraft number }",
    );
    assert_eq!(ready["data"]["markPullRequestReadyForReview"]["pullRequest"]["isDraft"], json!(false));
    let back = gql(&mut c, "convertPullRequestToDraft", json!({ "pullRequestId": pr_id }), "pullRequest { isDraft }");
    assert_eq!(back["data"]["convertPullRequestToDraft"]["pullRequest"]["isDraft"], json!(true));
    let issue_id = {
        let r = c.repo(0, &h, &dir);
        NodeId::scoped(NodeType::Issue, REPO, r.item(IssueNumber(1)).unwrap().row.id.0).encode()
    };
    let label_id = NodeId::scoped(NodeType::Label, REPO, wip.0).encode();
    let sel = "labelable { ... on Issue { labels(first: 10) { nodes { name } } } }";
    let added = gql(&mut c, "addLabelsToLabelable", json!({ "labelableId": issue_id, "labelIds": [label_id] }), sel);
    let names = |v: &Value, f: &str| -> Vec<String> {
        v["data"][f]["labelable"]["labels"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["name"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(&added, "addLabelsToLabelable"), ["bug", "wip"]);
    let removed =
        gql(&mut c, "removeLabelsFromLabelable", json!({ "labelableId": issue_id, "labelIds": [label_id] }), sel);
    assert_eq!(names(&removed, "removeLabelsFromLabelable"), ["bug"]);
    // The edge finds the repository from the request alone (`route`), from the `labelableId` as from a
    // `pullRequestId`; the hosted B0-36 run got "a mutation runs through plan" before this.
    for field in ["addLabelsToLabelable", "removeLabelsFromLabelable"] {
        let q = format!("mutation($i: {}Input!) {{ {field}(input: $i) {{ clientMutationId }} }}", {
            let mut s = field.to_owned();
            s[..1].make_ascii_uppercase();
            s
        });
        let vars = json!({ "i": { "labelableId": issue_id, "labelIds": [label_id] } }).as_object().cloned();
        let req = Request::parse(&q, vars, None).unwrap();
        assert_eq!(route(&req).unwrap(), Some(graftyard_api::graphql::Target::Id(REPO.0.0)), "{field}");
    }

    // REST: add, replace, remove one, remove all.
    let path = "/repos/{owner}/{repo}/issues/{issue_number}/labels";
    let rest = |c: &mut Chain, m: &str, name: Option<&str>, body: Value| -> graftyard_api::wire::Reply {
        let op = {
            let r = c.repo(0, &h, &dir);
            plan_issue_labels(&r, m, "1", name, &body)
        };
        let op = match op {
            Ok(op) => op,
            Err(x) => return x,
        };
        c.commit(WRITER, op).unwrap();
        let r = c.repo(0, &h, &dir);
        issue_labels_ok(&r, 1, m == "DELETE" && name.is_none())
    };
    let label_names = |x: &graftyard_api::wire::Reply| -> Vec<String> {
        x.value().as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap().to_owned()).collect()
    };
    let a = rest(&mut c, "POST", None, json!({ "labels": ["wip"] }));
    assert_eq!((a.status, label_names(&a)), (200, vec!["bug".to_owned(), "wip".to_owned()]));
    keep_rest_as("b036-issue-labels-post", "POST", path, &a);
    let p = rest(&mut c, "PUT", None, json!(["wip"]));
    assert_eq!((p.status, label_names(&p)), (200, vec!["wip".to_owned()]));
    keep_rest_as("b036-issue-labels-put", "PUT", path, &p);
    let one = rest(&mut c, "DELETE", Some("wip"), Value::Null);
    assert_eq!((one.status, label_names(&one)), (200, Vec::<String>::new()));
    keep_rest_as("b036-issue-labels-delete-one", "DELETE", &format!("{path}/{{name}}"), &one);
    rest(&mut c, "POST", None, json!(["bug", { "name": "wip" }]));
    let all = rest(&mut c, "DELETE", None, Value::Null);
    assert_eq!(all.status, 204);
    keep_rest_as("b036-issue-labels-delete-all", "DELETE", path, &all);
    // An unknown name is 422 for POST, and 404 for removing a label the issue does not have.
    assert_eq!(rest(&mut c, "POST", None, json!(["nope"])).status, 422);
    // One unknown name refuses the whole request, beside a known one.
    assert_eq!(rest(&mut c, "POST", None, json!(["bug", "nope"])).status, 422);
    assert_eq!(rest(&mut c, "DELETE", Some("bug"), Value::Null).status, 404);
    assert_eq!(rest(&mut c, "POST", None, json!({ "labels": [] })).status, 422);
    let r = c.repo(0, &h, &dir);
    assert!(plan_issue_labels(&r, "POST", "99", None, &json!(["bug"])).is_err_and(|x| x.status == 404));
}

/// A tier too large to serve is refused after one byte budget's work, not after `ROWS_MAX` rows: B0-30's 4 GB
/// metadata tier was scanned to 50,000 rows a read only to answer `complete: false`.
#[test]
fn an_oversized_tier_read_stops_at_its_byte_budget() {
    use graftyard_api::hosted::{Budget, TierRows};
    use graftyard_fold::store::{Key, Read};
    use graftyard_fold::{Store, Stored, Val};
    use graftyard_git::tiers::METADATA;
    use std::cell::Cell;
    struct Counted<'a>(&'a MemStore, Cell<usize>);
    impl Read for Counted<'_> {
        fn get(&self, table: &str, key: &[Val]) -> Option<Vec<Val>> {
            Read::get(self.0, table, key)
        }
        fn scan(&self, table: &str, from: &[Val], limit: usize) -> Vec<(Key, Vec<Val>)> {
            let rows = Read::scan(self.0, table, from, limit);
            self.1.set(self.1.get() + rows.len());
            rows
        }
    }
    let mut c = seeded();
    let m = &mut c.members[0];
    for i in 0..3_000u32 {
        let row = Stored { written: graftyard_types::Index(1), cols: vec![Val::Blob(vec![9; 64 << 10].into())] };
        m.meta.put("path_set", vec![Val::Int(i64::from(i))], row.clone());
        m.meta.put("comment", vec![Val::Int((1 << 40) + i64::from(i))], row);
    }
    let counted = Counted(&c.members[0].meta, Cell::new(0));
    let mut budget = Budget::READ;
    let (rows, complete) = TierRows::dump(&counted, &METADATA, &mut budget);
    assert!(!complete);
    let bytes: usize = rows
        .iter()
        .map(|(_, _, cols)| {
            cols.iter()
                .map(|v| match v {
                    Val::Blob(b) => b.len(),
                    _ => 0,
                })
                .sum::<usize>()
        })
        .sum();
    assert!(bytes <= Budget::READ.bytes && bytes > Budget::READ.bytes / 2, "{bytes}");
    // The scan stopped within one batch of the budget, far short of the 3,000 large comments it holds.
    assert!(counted.1.get() < 1_200, "{} rows scanned", counted.1.get());
    // A small tier still reads whole under the same budget.
    let (_, small) = TierRows::dump(&seeded().members[0].meta, &METADATA, &mut Budget::READ.clone());
    assert!(small);
}

#[test]
fn rest_reads_of_the_trust_resources_render_ghes_shapes() {
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.push("refs/heads/f", oid(20));
    c.push("refs/heads/other", oid(3));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    c.commit(
        READER,
        IssueOp::OpenIssue {
            title: "an issue".into(),
            body: "issue body".into(),
            labels: SmallVec::new(),
            assignees: SmallVec::new(),
        },
    )
    .unwrap();
    let open_pr = |branch: &str, title: &str, head_sha| IssueOp::OpenPr {
        head: graftyard_git::issues::HeadSpec { repo: REPO, branch: name(branch) },
        base: name("refs/heads/main"),
        title: title.into(),
        body: "pull body".into(),
        draft: false,
        head_sha,
        base_sha: oid(1),
    };
    // The head carries runs 40 and 41 below, so the GraphQL rollup can render their annotations too.
    c.commit(WRITER, open_pr("refs/heads/f", "a pull request", oid(20))).unwrap();
    let id = |out: IssueOut| match out {
        IssueOut::Id(id) => id.0,
        other => panic!("{other:?}"),
    };
    let first = id(c
        .commit(
            READER,
            IssueOp::Comment {
                on: CommentTarget::Issue(IssueNumber(1)),
                body: "first issue comment".into(),
                reply_to: None,
            },
        )
        .unwrap());
    let second = id(c
        .commit(
            OWNER,
            IssueOp::Comment {
                on: CommentTarget::Issue(IssueNumber(1)),
                body: "second issue comment".into(),
                reply_to: None,
            },
        )
        .unwrap());
    let review = id(c
        .commit(
            OWNER,
            IssueOp::Review {
                n: IssueNumber(2),
                state: ReviewState::Commented,
                commit: oid(2),
                body: "review body".into(),
            },
        )
        .unwrap());
    let review_op = |state, body: &str| IssueOp::Review { n: IssueNumber(2), state, commit: oid(2), body: body.into() };
    c.commit(READER, review_op(ReviewState::Pending, "pending draft")).unwrap();
    let approval = id(c.commit(OWNER, review_op(ReviewState::Approved, "approved then dismissed")).unwrap());
    c.commit(
        OWNER,
        IssueOp::Dismiss { review: graftyard_git::issues::ReviewId(DbId(approval)), message: "stale".into() },
    )
    .unwrap();
    let at = |line| LineAnchor {
        commit: oid(2),
        path: "src/lib.rs".into(),
        line,
        start_line: (line == 8).then_some(7),
        side: if line == 8 { DiffSide::Right } else { DiffSide::Left },
    };
    let comment = id(c
        .commit(
            READER,
            IssueOp::Comment {
                on: CommentTarget::Review { n: IssueNumber(2), review: None, at: at(8) },
                body: "first review comment".into(),
                reply_to: None,
            },
        )
        .unwrap());
    c.now = Timestamp(c.now.0 + 1_000);
    let reply = id(c
        .commit(
            OWNER,
            IssueOp::Comment {
                on: CommentTarget::Review { n: IssueNumber(2), review: None, at: at(12) },
                body: "reply review comment".into(),
                reply_to: Some(graftyard_git::issues::CommentId(DbId(comment))),
            },
        )
        .unwrap());
    c.commit(WRITER, open_pr("refs/heads/other", "another pull request", oid(3))).unwrap();
    let elsewhere = id(c
        .commit(
            READER,
            IssueOp::Comment {
                on: CommentTarget::Review {
                    n: IssueNumber(3),
                    review: None,
                    at: LineAnchor {
                        commit: oid(3),
                        path: "elsewhere.rs".into(),
                        line: 1,
                        start_line: None,
                        side: DiffSide::Right,
                    },
                },
                body: "other pull request comment".into(),
                reply_to: None,
            },
        )
        .unwrap());
    c.commit(WRITER, run(40, 20, "build", Some(CheckConclusion::Success))).unwrap();
    let mut detailed = run(41, 20, "test", Some(CheckConclusion::Failure));
    let IssueOp::RecordCheck(update) = &mut detailed else { unreachable!() };
    update.output = Some(CheckOutput {
        title: "test output".into(),
        summary: "one failure".into(),
        text: Some("details".into()),
        annotations: vec![
            CheckAnnotation {
                path: ".github".into(),
                start_line: 9,
                end_line: 9,
                level: AnnotationLevel::Failure,
                message: "boom".into(),
                title: Some("t1".into()),
            },
            CheckAnnotation {
                path: "src/a.rs".into(),
                start_line: 3,
                end_line: 5,
                level: AnnotationLevel::Warning,
                message: "warn".into(),
                title: None,
            },
        ],
    });
    c.commit(WRITER, detailed).unwrap();
    c.commit(WRITER, run(42, 21, "other", Some(CheckConclusion::Success))).unwrap();

    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let pull = get_pull(&r, "2");
    assert_eq!((pull.status, &pull.value()["number"]), (200, &json!(2)));
    keep_rest("trust-pull", "/repos/{owner}/{repo}/pulls/{pull_number}", &pull);
    assert_eq!(get_pull(&r, "99").status, 404);
    assert_eq!(get_pull(&r, "not-a-number").status, 404);
    assert_eq!(get_pull(&r, "1").status, 404, "an issue is not a pull request");
    assert_eq!(list_reviews(&r, "1", "").status, 404);
    assert_eq!(list_review_comments(&r, "1", "").status, 404);

    let comments = list_issue_comments(&r, "1", "");
    let got: Vec<(i64, &str)> = comments
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_i64().unwrap(), c["body"].as_str().unwrap()))
        .collect();
    assert_eq!(got, [(first, "first issue comment"), (second, "second issue comment")]);
    keep_rest("trust-issue-comments", "/repos/{owner}/{repo}/issues/{issue_number}/comments", &comments);
    assert_eq!(list_issue_comments(&r, "99", "").status, 404);

    // A pending review is the reviewer's own draft and not listed; a dismissed approval reads DISMISSED. Each review
    // comment made outside a review, a reply too, is its own single-comment review, COMMENTED with no body, as GHES
    // makes one; its id is the comment's `pull_request_review_id`.
    let review_comments = list_review_comments(&r, "2", "");
    let review_of = |id: i64| {
        let objects = review_comments.value().as_array().unwrap();
        objects.iter().find(|c| c["id"] == id).unwrap()["pull_request_review_id"].as_i64()
    };
    let (single, single_reply) = (review_of(comment).expect("a review"), review_of(reply).expect("a review"));
    let reviews = list_reviews(&r, "2", "");
    let got: Vec<(i64, &str, &str)> = reviews
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["id"].as_i64().unwrap(), v["body"].as_str().unwrap(), v["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        got,
        [
            (review, "review body", "COMMENTED"),
            (approval, "approved then dismissed", "DISMISSED"),
            (single, "", "COMMENTED"),
            (single_reply, "", "COMMENTED")
        ]
    );
    keep_rest("trust-reviews", "/repos/{owner}/{repo}/pulls/{pull_number}/reviews", &reviews);

    let objects = review_comments.value().as_array().unwrap();
    let got: Vec<(i64, &str)> =
        objects.iter().map(|c| (c["id"].as_i64().unwrap(), c["body"].as_str().unwrap())).collect();
    // Oldest first, though the reply's Metadata id is lower than its thread's Core id.
    assert!(reply < comment, "the ids no longer test the order");
    assert_eq!(got, [(comment, "first review comment"), (reply, "reply review comment")]);
    let first_object = objects.iter().find(|c| c["id"] == comment).unwrap();
    let reply_object = objects.iter().find(|c| c["id"] == reply).unwrap();
    assert_eq!(reply_object["in_reply_to_id"], comment);
    assert!(first_object.get("in_reply_to_id").is_none());
    assert!(!objects.iter().any(|c| c["id"] == elsewhere), "another pull request's comment leaked in");
    assert_eq!(first_object["start_line"], 7);
    assert_eq!(first_object["start_side"], "RIGHT");
    assert!(reply_object["start_line"].is_null() && reply_object["start_side"].is_null());
    keep_rest("trust-review-comments", "/repos/{owner}/{repo}/pulls/{pull_number}/comments", &review_comments);

    let checks = list_check_runs_for_ref(&r, &oid(20).hex(), "");
    assert_eq!(checks.value()["total_count"], 2);
    let runs = checks.value()["check_runs"].as_array().unwrap();
    let ids: Vec<i64> = runs.iter().map(|run| run["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [41, 40], "runs are newest id first and the other sha's run is absent");
    assert_eq!(runs[0]["output"]["title"], "test output");
    keep_rest("trust-check-runs", "/repos/{owner}/{repo}/commits/{ref}/check-runs", &checks);
    let one = get_check_run(&r, "41");
    assert_eq!(
        (one.status, &one.value()["id"], &one.value()["output"]["summary"]),
        (200, &json!(41), &json!("one failure"))
    );
    keep_rest("trust-check-run", "/repos/{owner}/{repo}/check-runs/{check_run_id}", &one);
    assert_eq!(one.value()["output"]["annotations_count"], 2);
    assert_eq!(get_check_run(&r, "999999").status, 404);
    let missing = list_check_runs_for_ref(&r, "does-not-exist", "");
    assert_eq!(missing.status, 422);
    assert_eq!(missing.value()["message"], "No commit found for SHA: does-not-exist");
    assert_eq!(commit_not_found("does-not-exist").value()["message"], "No commit found for SHA: does-not-exist");

    let sha = oid(20).hex();
    let expected = json!([
        {
            "path": ".github",
            "blob_href": format!("{}/o/r/blob/{sha}/.github", r.host.web()),
            "start_line": 9,
            "end_line": 9,
            "start_column": null,
            "end_column": null,
            "annotation_level": "failure",
            "title": "t1",
            "message": "boom",
            "raw_details": null,
        },
        {
            "path": "src/a.rs",
            "blob_href": format!("{}/o/r/blob/{sha}/src/a.rs", r.host.web()),
            "start_line": 3,
            "end_line": 5,
            "start_column": null,
            "end_column": null,
            "annotation_level": "warning",
            "title": null,
            "message": "warn",
            "raw_details": null,
        },
    ]);
    let annotations = list_check_run_annotations(&r, "41", "");
    assert_eq!(annotations.status, 200);
    assert_eq!(annotations.value(), &expected);
    assert_eq!(list_check_run_annotations(&r, "40", "").value(), &json!([]), "no output means no annotations");
    let page = list_check_run_annotations(&r, "41", "per_page=1&page=2");
    assert_eq!(page.status, 200);
    assert_eq!(page.value(), &json!([expected[1].clone()]));
    let url =
        |page| format!("{}/repositories/{}/check-runs/41/annotations?per_page=1&page={page}", r.host.api(), REPO.0.0);
    assert_eq!(page.header("link"), Some(format!("<{}>; rel=\"prev\", <{}>; rel=\"first\"", url(1), url(1)).as_str()));
    assert_eq!(list_check_run_annotations(&r, "999999", "").status, 404);
    assert_eq!(list_check_run_annotations(&r, "not-a-number", "").status, 404);

    let d = Doc {
        name: "CheckRunAnnotations".into(),
        query: r#"query($id: ID!) {
            node(id: $id) {
                ... on PullRequest {
                    commits(last: 1) {
                        nodes {
                            commit {
                                statusCheckRollup {
                                    contexts(first: 10) {
                                        nodes {
                                            ... on CheckRun {
                                                name
                                                annotations(first: 10) {
                                                    totalCount
                                                    nodes {
                                                        annotationLevel path title message
                                                        location { start { line column } end { line } }
                                                        blobUrl
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#
        .into(),
        variables: json!({ "id": "PR_1" }).as_object().cloned(),
    };
    let v = ask(&r, &d, with_ids(&d, &r));
    assert!(v.get("errors").is_none(), "{v}");
    let contexts = v["data"]["node"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .unwrap();
    let test = contexts.iter().find(|n| n["name"] == "test").unwrap();
    assert_eq!(test["annotations"]["totalCount"], 2);
    assert_eq!(
        test["annotations"]["nodes"],
        json!([
            {
                "annotationLevel": "FAILURE",
                "path": ".github",
                "title": "t1",
                "message": "boom",
                "location": { "start": { "line": 9, "column": null }, "end": { "line": 9 } },
                "blobUrl": format!("{}/o/r/blob/{sha}/.github", r.host.web()),
            },
            {
                "annotationLevel": "WARNING",
                "path": "src/a.rs",
                "title": null,
                "message": "warn",
                "location": { "start": { "line": 3, "column": null }, "end": { "line": 5 } },
                "blobUrl": format!("{}/o/r/blob/{sha}/src/a.rs", r.host.web()),
            },
        ])
    );

    let tree = oid(30);
    let parents = [oid(31), oid(32)];
    let raw = format!(
        "tree {}\nparent {}\nparent {}\nauthor A. U. Thor <author@example.com> 1700000000 +0530\ncommitter Committer <committer@example.com> 1700000060 -0700\ngpgsig -----BEGIN SIGNATURE-----\n signature continuation\n -----END SIGNATURE-----\n\ncommit subject\n\ncommit body\n",
        tree.hex(),
        parents[0].hex(),
        parents[1].hex(),
    );
    let commit = get_commit(&r, oid(29), raw.as_bytes());
    assert_eq!(commit.status, 200);
    assert_eq!(commit.value()["commit"]["message"], "commit subject\n\ncommit body");
    assert_eq!(commit.value()["commit"]["author"]["name"], "A. U. Thor");
    assert_eq!(commit.value()["commit"]["author"]["email"], "author@example.com");
    assert_eq!(commit.value()["commit"]["author"]["date"], "2023-11-14T22:13:20Z");
    assert_eq!(commit.value()["commit"]["tree"]["sha"], tree.hex());
    let got: Vec<&str> =
        commit.value()["parents"].as_array().unwrap().iter().map(|p| p["sha"].as_str().unwrap()).collect();
    assert_eq!(got, [parents[0].hex(), parents[1].hex()]);
    keep_rest("trust-commit", "/repos/{owner}/{repo}/commits/{ref}", &commit);

    // Under the agent media type each route's objects get the view of the text and the author it renders: a
    // reader's comment is untrusted and the owner's trusted, so a renderer that dropped `user` would show.
    let agent = Some(graftyard_api::trust::AGENT_MEDIA_TYPE);
    let views = |path: &str, reply: &graftyard_api::wire::Reply| -> Vec<(Value, Value)> {
        let on = graftyard_api::trust::layer(agent, path, &r, reply.clone());
        let v = on.value().clone();
        let objs = match &v {
            Value::Array(xs) => xs.clone(),
            Value::Object(m) if m.contains_key("check_runs") => m["check_runs"].as_array().unwrap().clone(),
            o => vec![o.clone()],
        };
        objs.iter()
            .map(|o| (o["graftyard_trust"]["visible"].clone(), o["graftyard_trust"]["author"]["trusted"].clone()))
            .collect()
    };
    let t = |s: &str, trusted: bool| (json!(s), json!(trusted));
    assert_eq!(views("/repos/o/r/pulls/2", &pull), [t("pull body", true)]);
    assert_eq!(
        views("/repos/o/r/issues/1/comments", &comments),
        [t("first issue comment", false), t("second issue comment", true)]
    );
    assert_eq!(
        views("/repos/o/r/pulls/2/reviews", &reviews),
        [t("review body", true), t("approved then dismissed", true), t("", false), t("", true)]
    );
    assert_eq!(
        views("/repos/o/r/pulls/2/comments", &review_comments),
        [t("first review comment", false), t("reply review comment", true)]
    );
    assert_eq!(
        views(&format!("/repos/o/r/commits/{}/check-runs", oid(20).hex()), &checks)[0],
        t("test output\n\none failure\n\ndetails", false)
    );
    assert_eq!(views("/repos/o/r/check-runs/41", &one), [t("test output\n\none failure\n\ndetails", false)]);
    assert_eq!(views("/repos/o/r/commits/x", &commit), [t("commit subject\n\ncommit body", false)]);
}

#[test]
fn actions_empty_check_run_output_title_and_summary_are_null() {
    let mut c = seeded();
    let empty = |id, app| {
        let mut op = run(id, 2, "job", Some(CheckConclusion::Success));
        let IssueOp::RecordCheck(update) = &mut op else { unreachable!() };
        update.app = app;
        update.output = Some(CheckOutput { title: "".into(), summary: "".into(), text: None, annotations: vec![] });
        op
    };
    c.commit(WRITER, empty(43, graftyard_git::run_checks::ACTIONS_APP)).unwrap();
    c.commit(WRITER, empty(44, AppId(DbId(5)))).unwrap();
    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);
    let actions = get_check_run(&r, "43");
    assert!(actions.value()["output"]["title"].is_null());
    assert!(actions.value()["output"]["summary"].is_null());
    let other = get_check_run(&r, "44");
    assert_eq!(other.value()["output"]["title"], "");
    assert_eq!(other.value()["output"]["summary"], "");
}

/// lf4-rest-2: `POST pulls/{n}/comments` and `POST check-runs` plan a command from GHES 3.22's body, commit it, and
/// render the pinned 201 from the tiers after the entry applied, the way the member does (`hosted::render_ok`).
#[test]
fn rest_writes_of_review_comments_and_check_runs_render_ghes_shapes() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_err, render_ok};
    use graftyard_api::rest::{
        check_run_created, plan_create_check_run, plan_create_review_comment, review_comment_created, write_refused,
    };
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(READER));
    let sha = oid(2).hex();
    let plan_rc = |c: &Chain, n: &str, body: Value| {
        let r = c.repo(0, &h, &dir);
        plan_create_review_comment(&r, n, &body)
    };

    // A new thread on pull #2 by a reader, over lines 5 to 7 on the right.
    let body = json!({ "body": "rename this", "commit_id": sha, "path": "src/a.rs", "line": 7, "start_line": 5,
        "side": "RIGHT", "start_side": "RIGHT" });
    let op = plan_rc(&c, "2", body).unwrap();
    let first = c.commit(READER, op).unwrap();
    let r = c.repo(0, &h, &dir);
    let created = review_comment_created(&ack(first.clone()), &r, 2);
    let v = created.value();
    assert_eq!(created.status, 201);
    assert_eq!(
        (&v["body"], &v["path"], &v["line"], &v["start_line"], &v["side"], &v["start_side"], &v["commit_id"]),
        (
            &json!("rename this"),
            &json!("src/a.rs"),
            &json!(7),
            &json!(5),
            &json!("RIGHT"),
            &json!("RIGHT"),
            &json!(sha)
        )
    );
    assert_eq!((&v["user"]["login"], v.get("in_reply_to_id")), (&json!("bob"), None));
    let parent = v["id"].as_i64().unwrap();
    keep_rest_as("review-comment-created", "POST", "/repos/{owner}/{repo}/pulls/{pull_number}/comments", &created);
    let listed = list_review_comments(&r, "2", "");
    assert!(listed.value().as_array().unwrap().iter().any(|x| x["id"] == json!(parent)));
    drop(r);

    // A reply takes its parent's anchor, whatever line, path, or side the body names.
    let body = json!({ "body": "done", "in_reply_to": parent, "path": "other.rs", "line": 99, "side": "LEFT" });
    let out = c.commit(WRITER, plan_rc(&c, "2", body).unwrap()).unwrap();
    let r = c.repo(0, &h, &dir);
    let reply = review_comment_created(&ack(out), &r, 2);
    let v = reply.value();
    assert_eq!(reply.status, 201);
    assert_eq!(
        (&v["in_reply_to_id"], &v["path"], &v["line"], &v["start_line"], &v["side"]),
        (&json!(parent), &json!("src/a.rs"), &json!(7), &json!(5), &json!("RIGHT"))
    );
    keep_rest_as("review-comment-reply", "POST", "/repos/{owner}/{repo}/pulls/{pull_number}/comments", &reply);
    let issue_comment = r.comments_of(IssueNumber(1)).first().map(|(id, _)| id.0.0).unwrap();
    drop(r);

    // Refusals before any command.
    let ok = || json!({ "body": "x", "commit_id": sha, "path": "a", "line": 3 });
    let with = |k: &str, v: Value| {
        let mut b = ok();
        if v.is_null() {
            b.as_object_mut().unwrap().remove(k);
        } else {
            b[k] = v;
        }
        b
    };
    let status = |n: &str, b: Value| plan_rc(&c, n, b).err().map(|r| r.status);
    assert_eq!(status("2", ok()), None);
    assert_eq!(status("1", ok()), Some(404), "issue #1 is not a pull request");
    assert_eq!(status("99", ok()), Some(404));
    assert_eq!(status("2", with("commit_id", Value::Null)), Some(422));
    assert_eq!(status("2", with("commit_id", json!("abc"))), Some(422));
    assert_eq!(status("2", with("path", Value::Null)), Some(422));
    assert_eq!(status("2", with("body", json!(""))), Some(422));
    assert_eq!(
        status("2", json!({ "body": "x", "commit_id": sha, "path": "a", "position": 3 })),
        Some(422),
        "position only"
    );
    assert_eq!(status("2", with("side", json!("MIDDLE"))), Some(422));
    assert_eq!(status("2", with("start_line", json!(3))), Some(422), "start_line must be below line");
    assert_eq!(status("2", with("start_line", json!(2))), None);
    assert_eq!(status("2", with("start_side", json!("LEFT"))), Some(422), "start_side on another side");
    assert_eq!(status("2", with("subject_type", json!("file"))), Some(422));
    assert_eq!(status("2", json!({ "body": "x", "in_reply_to": 424242 })), Some(404));
    assert_eq!(status("2", json!({ "body": "x", "in_reply_to": issue_comment })), Some(404), "an issue comment");
    let left = plan_rc(&c, "2", ok()).unwrap();
    let IssueOp::Comment { on: CommentTarget::Review { at, .. }, .. } = left else { panic!() };
    assert_eq!(at.side, DiffSide::Right, "side defaults to RIGHT");

    // A check run by App 5, with a hidden character in its summary.
    let now = Timestamp(1_800_000_000_000);
    let body = json!({ "name": "lint2", "head_sha": sha, "status": "in_progress", "started_at": "2026-11-12T01:00:00Z",
        "details_url": "https://ci.example/7", "external_id": "e7",
        "output": { "title": "t", "summary": "s\u{200b}", "annotations": [
            { "path": "a.rs", "start_line": 1, "end_line": 2, "annotation_level": "warning", "message": "m" } ] } });
    // Two creates in one entry, then one in the next: `decide` hands each its own id from the metadata tier's cursor,
    // whatever id the plan carried, and the run's suite takes the same id.
    let op = plan_create_check_run(&body, AppId(DbId(5)), now).unwrap();
    let mut other = op.clone();
    let IssueOp::CreateCheck(u) = &mut other else { panic!("a create") };
    u.name = "lint3".into();
    let outs = c.commit_all(vec![(WRITER, op.clone()), (WRITER, other)]);
    let ids: Vec<DbId> = outs
        .iter()
        .map(|o| match o {
            Ok(IssueOut::Id(id)) => *id,
            o => panic!("{o:?}"),
        })
        .collect();
    let Ok(IssueOut::Id(third)) = c.commit(WRITER, op.clone()) else { panic!("a third create") };
    assert!(ids[0] != ids[1] && third != ids[0] && third != ids[1], "distinct ids: {ids:?} and {third:?}");
    let block = |id: DbId| (id.cell(), id.block());
    assert!(ids.iter().chain([&third]).all(|id| block(*id) == (0, 1) && id.0 > 0), "from the leased block: {ids:?}");
    let run_id = ids[0];
    let r = c.repo(0, &h, &dir);
    let run = check_run_created(&ack(IssueOut::Id(run_id)), &r, 0);
    let v = run.value();
    assert_eq!(run.status, 201);
    assert_eq!(
        (&v["check_suite"]["id"], &check_run_created(&ack(IssueOut::Id(ids[1])), &r, 0).value()["name"]),
        (&json!(run_id.0), &json!("lint3"))
    );
    assert_eq!(
        (&v["id"], &v["name"], &v["status"], &v["conclusion"], &v["started_at"], &v["completed_at"]),
        (
            &json!(run_id.0),
            &json!("lint2"),
            &json!("in_progress"),
            &Value::Null,
            &json!("2026-11-12T01:00:00Z"),
            &Value::Null
        )
    );
    assert_eq!((&v["output"]["annotations_count"], &v["external_id"]), (&json!(1), &json!("e7")));
    keep_rest_as("check-run-created", "POST", "/repos/{owner}/{repo}/check-runs", &run);
    assert_eq!(get_check_run(&r, &run_id.0.to_string()).value()["id"], json!(run_id.0));
    drop(r);

    // A conclusion completes the run at `now`, as GitHub does.
    let op = plan_create_check_run(
        &json!({ "name": "done", "head_sha": sha, "conclusion": "success" }),
        AppId(DbId(5)),
        now,
    )
    .unwrap();
    let IssueOp::CreateCheck(u) = &op else { panic!() };
    assert_eq!(
        (u.status, u.conclusion, u.completed, u.started),
        (CheckStatus::Completed, Some(CheckConclusion::Success), Some(now), Some(now))
    );
    let bad = |b: Value| plan_create_check_run(&b, AppId(DbId(5)), now).err().map(|r| r.status);
    let base = || json!({ "name": "n", "head_sha": sha });
    let mut b = base();
    b["status"] = json!("completed");
    assert_eq!(bad(b), Some(422), "completed without a conclusion");
    assert_eq!(bad(json!({ "name": "n", "head_sha": "abc" })), Some(422));
    assert_eq!(bad(json!({ "head_sha": sha })), Some(422));
    let mut b = base();
    b["started_at"] = json!("yesterday");
    assert_eq!(bad(b), Some(422));
    let mut b = base();
    b["conclusion"] = json!("great");
    assert_eq!(bad(b), Some(422));
    let mut b = base();
    b["output"] = json!({ "title": "t" });
    assert_eq!(bad(b), Some(422), "output without summary");
    let mut b = base();
    let one = json!({ "path": "a", "start_line": 1, "end_line": 1, "annotation_level": "notice", "message": "m" });
    b["output"] = json!({ "title": "t", "summary": "s", "annotations": vec![one; 51] });
    assert_eq!(bad(b), Some(422), "more than 50 annotations");

    // A repository with no ids left refuses a create until the lease loop records a block: 503, never a minted id.
    assert_eq!(write_refused(&IssueErr::NeedIdBlock, "rest/checks/runs#create-a-check-run").status, 503);
    // A later update of a created run, as `RecordCheck` version 2 under the id the create returned, replaces it.
    let IssueOp::CreateCheck(mut u) = op else { panic!() };
    (u.run, u.suite, u.version) =
        (graftyard_git::issues::CheckRunId(run_id), graftyard_git::issues::CheckSuiteId(run_id), 2);
    c.commit(WRITER, IssueOp::RecordCheck(u)).unwrap();
    assert_eq!(get_check_run(&c.repo(0, &h, &dir), &run_id.0.to_string()).value()["status"], json!("completed"));

    // The member's render, through the `What` each route sends, equals the in-process one.
    let hosted = HostedDir { home: Some(dir.home()), viewer: Some(AccountId(DbId(READER))), ..Default::default() };
    let render = |what: What| -> Render {
        let r = Render { host: h.name().to_owned(), isolation: false, dir: hosted.clone(), what };
        serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap()
    };
    let m = &c.members[0];
    let wire = render_ok(&render(What::CheckRunCreated { id: 0 }), &ack(IssueOut::Id(ids[1])), &m.core, &m.meta);
    assert_eq!((wire.status, wire.body.as_ref().map(|b| &b["id"])), (201, Some(&json!(ids[1].0))), "the acked id");
    let wire = render_ok(&render(What::CheckRunCreated { id: 0 }), &ack(IssueOut::Done), &m.core, &m.meta);
    assert_eq!(wire.status, 500, "no id acked and none minted");
    let wire = render_ok(&render(What::ReviewCommentCreated { n: 2 }), &ack(first), &m.core, &m.meta);
    assert_eq!((wire.status, wire.body.as_ref().map(|b| &b["id"])), (201, Some(&json!(parent))));
    let e = render_err(&render(What::ReviewCommentCreated { n: 2 }), &IssueErr::NotPull);
    assert_eq!(e.status, 404);
    let e = render_err(&render(What::CheckRunCreated { id: 1 }), &IssueErr::Forbidden);
    assert_eq!(e.status, 403);
    // An Actions secret or variable write: a new name is 201 with a JSON body (go-gh decodes every non-204 reply, so
    // an empty 201 fails `gh secret set`), an update is 204, and a variable create on a taken name is 409, which
    // `gh variable set` answers with a PATCH.
    let wire = render_ok(&render(What::ActionsConfig), &ack(IssueOut::Created), &m.core, &m.meta);
    assert_eq!((wire.status, wire.body), (201, Some(json!({}))));
    let wire = render_ok(&render(What::ActionsConfig), &ack(IssueOut::Done), &m.core, &m.meta);
    assert_eq!((wire.status, wire.body), (204, None));
    assert_eq!(render_err(&render(What::ActionsConfig), &IssueErr::Exists).status, 409);
}

/// A pull request's `commits`, `additions`, `deletions`, and `changed_files` are the git cache's counts for its (base
/// tip, head), in REST and GraphQL alike: a render over a pair the edge has not read counts 0 and records the pair in
/// `asked`, and once the edge stores the shard's counts the next render shows them. The base tip is the branch's
/// current one, not the one the pull request was opened on.
#[test]
fn a_pull_requests_counts_are_the_git_caches_for_its_base_tip_and_head() {
    use graftyard_api::hosted::HostedDir;
    use graftyard_api::view::Compare;
    let mut c = seeded();
    c.push("refs/heads/main", oid(7));
    let (h, dir) = (host(), Dir::new(WRITER));
    let hosted = HostedDir { home: Some(dir.home()), viewer: Some(AccountId(DbId(WRITER))), ..Default::default() };
    let m = &c.members[0];
    let r = Repo { host: &h, at: dir.home(), core: &m.core, meta: &m.meta, dir: &hosted };
    let counts =
        |v: &Value| (v["commits"].clone(), v["additions"].clone(), v["deletions"].clone(), v["changed_files"].clone());
    let before = get_pull(&r, "2");
    assert_eq!(counts(before.value()), (json!(0), json!(0), json!(0), json!(0)));
    assert_eq!(*hosted.asked.borrow(), vec![(oid(7), oid(2))], "the current base tip, not the opening one");
    hosted
        .compares
        .borrow_mut()
        .push(((oid(7), oid(2)), Compare { commits: 2, additions: 5, deletions: 3, changed_files: 4 }));
    let after = get_pull(&r, "2");
    assert_eq!(counts(after.value()), (json!(2), json!(5), json!(3), json!(4)));
    let q = r#"query { repository(owner: "o", name: "r") { pullRequest(number: 2) {
        additions deletions commits { totalCount } } } }"#;
    let v = query(&Request::parse(q, None, None).unwrap(), Some(&r));
    let pr = &v["data"]["repository"]["pullRequest"];
    assert_eq!((&pr["additions"], &pr["deletions"], &pr["commits"]["totalCount"]), (&json!(5), &json!(3), &json!(2)));
}

/// `rest:branches`' branch protection, as BF-56 seeds its rule: a `PUT` on a branch with a rule edits that rule rather
/// than adding a second, its `contexts` and `checks` both become required names, the reply and a `GET` render the rule
/// the entry left, and a `DELETE` leaves the branch unprotected.
#[test]
fn branch_protection_put_edits_the_branch_s_rule_and_delete_removes_it() {
    use graftyard_api::protection::{get, plan_delete, plan_put, saved};
    let mut c = seeded();
    let (h, dir) = (host(), Dir::new(OWNER));
    let before = c.repo(0, &h, &dir).rules();
    assert_eq!(before.len(), 1, "seeded's rule on main");
    let body = json!({
        "required_status_checks": { "strict": true, "contexts": ["build"], "checks": [{ "context": "test", "app_id": 2 }] },
        "enforce_admins": true, "required_pull_request_reviews": null, "restrictions": null,
    });
    let op = plan_put(&c.repo(0, &h, &dir), "main", &body).unwrap();
    let IssueOut::Id(id) = c.commit(OWNER, op).unwrap() else { panic!() };
    let r = c.repo(0, &h, &dir);
    assert_eq!((r.rules().len(), id), (1, before[0].id), "the same rule, edited");
    assert!(r.is_required(&name("refs/heads/main"), "build", Some(AppId(DbId(9)))));
    assert!(r.is_required(&name("refs/heads/main"), "test", Some(AppId(DbId(2)))));
    assert!(!r.is_required(&name("refs/heads/main"), "test", Some(AppId(DbId(9)))), "test only from App 2");
    assert!(!r.is_required(&name("refs/heads/main"), "ci", None), "the PUT replaced the old checks");
    let reply = saved(&ack(IssueOut::Id(id)), &r, "main");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.value()["required_status_checks"]["contexts"], json!(["build", "test"]));
    assert_eq!(reply.value()["enforce_admins"]["enabled"], json!(true));
    assert_eq!(get(&r, "main").value(), reply.value());
    // A body missing one of GitHub's four required keys, or naming a wildcard branch, is 422 before any commit.
    assert_eq!(plan_put(&r, "main", &json!({ "required_status_checks": null })).unwrap_err().status, 422);
    assert_eq!(plan_put(&r, "rel*", &body).unwrap_err().status, 422);
    assert_eq!(get(&r, "f").status, 404);

    let op = plan_delete(&r, "main").unwrap();
    let out = c.commit(OWNER, op).unwrap();
    let r = c.repo(0, &h, &dir);
    assert_eq!(saved(&ack(out), &r, "main").status, 204);
    assert_eq!(get(&r, "main").status, 404);
    assert!(!r.is_required(&name("refs/heads/main"), "build", None));
    assert_eq!(plan_delete(&r, "main").unwrap_err().status, 404);
    // A writer's PUT is refused by the leader, as `SetRule` needs admin.
    let op = plan_put(&r, "main", &body).unwrap();
    assert!(matches!(c.commit(WRITER, op), Err(IssueErr::Forbidden)));

    // BF-37: `merge_queue` sets the rule's queue from GHES's ruleset parameters; a later PUT without the key keeps it,
    // `null` removes it, and a parameter out of GHES's bounds is 422.
    let mq = json!({ "check_response_timeout_minutes": 10, "grouping_strategy": "ALLGREEN", "max_entries_to_build": 4,
        "min_entries_to_merge": 1, "max_entries_to_merge": 3, "min_entries_to_merge_wait_minutes": 0, "merge_method": "SQUASH" });
    let mut with = body.clone();
    with["merge_queue"] = mq;
    let put = |c: &mut Chain, b: &Value| {
        let op = plan_put(&c.repo(0, &h, &dir), "main", b);
        op.map(|op| c.commit(OWNER, op).unwrap()).map_err(|r| r.status)
    };
    let queue = |c: &Chain| graftyard_git::queue::queue_settings(&c.repo(0, &h, &dir).rules());
    put(&mut c, &with).unwrap();
    let q = queue(&c).expect("a queue");
    assert_eq!((q.max_build, q.max_merge, q.min_merge, q.check_timeout_minutes), (4, 3, 1, 10));
    assert_eq!((q.grouping, q.method), (Grouping::AllGreen, MergeMethod::Squash));
    put(&mut c, &body).unwrap();
    assert!(queue(&c).is_some(), "a PUT without merge_queue keeps the queue");
    with["merge_queue"]["max_entries_to_build"] = json!(101);
    assert_eq!(put(&mut c, &with), Err(422));
    with["merge_queue"] = Value::Null;
    put(&mut c, &with).unwrap();
    assert!(queue(&c).is_none(), "null removes it");
}

/// `write-identity`: a write whose token row the edge took from its cache is refused at `decide` once the token is
/// revoked, and the member renders that refusal as the edge renders a token it refuses at the front: HTTP 401 with
/// GHES's `basic-error` "Bad credentials" for every REST and API write the edge may take from the cache, and HTTP 401
/// with `{"message": "Bad credentials"}` for a GraphQL mutation. On main a GraphQL mutation answered it at 200, and a
/// label or issue-labels write at 403.
#[test]
fn a_revoked_tokens_write_is_refused_at_decide_with_the_fronts_401() {
    use graftyard_api::hosted::{HostedDir, Render, What, render_err, render_merge_err};
    let mut c = seeded();
    let label = |name: &str| IssueOp::DefineLabel {
        id: None,
        name: LabelName(name.into()),
        color: [0x11, 0x22, 0x33],
        description: None,
    };
    // The same write passes before the revocation, so the refusal below is the revocation's.
    assert!(c.commit(WRITER, label("before")).is_ok());
    c.revoke(WRITER);
    let refused = c.commit(WRITER, label("after")).expect_err("decide reads the revoked row");
    assert!(matches!(refused, IssueErr::Unauthorized(_)), "{refused:?}");
    // The owner's token is not revoked: only the revoked caller is refused.
    assert!(c.commit(OWNER, label("owner")).is_ok());

    let render =
        |what: What| Render { host: host().name().to_owned(), isolation: false, dir: HostedDir::default(), what };
    let rest = json!({ "message": "Bad credentials", "documentation_url":
        "https://docs.github.com/enterprise-server@3.22/rest", "status": "401" });
    for what in [
        What::LabelCreated,
        What::IssueLabels { n: 1, empty: false },
        What::ReviewCommentCreated { n: 2 },
        What::CheckRunCreated { id: 0 },
        What::ReleaseCreated,
        What::ReleaseUpdated,
        What::Deleted,
        What::StatusCreated { sha: oid(2), id: 1 },
        What::Protection { branch: "main".into() },
        What::AgentPolicy,
        What::Task { created: true, contents: true },
        What::Lease { created: true },
    ] {
        let w = render_err(&render(what.clone()), &refused);
        assert_eq!((w.status, w.body.as_ref()), (401, Some(&rest)), "{what:?}");
    }
    let gql = What::Graphql {
        query: "mutation { addComment { clientMutationId } }".into(),
        variables: Value::Null,
        field: "addComment".into(),
        subject: Some(1),
    };
    let bad = json!({ "message": "Bad credentials", "status": "401" });
    let w = render_err(&render(gql.clone()), &refused);
    assert_eq!((w.status, w.body.as_ref()), (401, Some(&bad)));
    let merge = graftyard_git::merge::MergeError::Unauthorized(graftyard_identity::CredentialRefusal::Revoked);
    let w = render_merge_err(&render(gql.clone()), &merge);
    assert_eq!((w.status, w.body.as_ref()), (401, Some(&bad)));
    // Any other refusal of a mutation keeps GraphQL's error shape at 200.
    assert_eq!(render_err(&render(gql), &IssueErr::NotFound).status, 200);
}

/// The web app's chrome reads (`decisions/OPEN-QUESTIONS.md`, F61): branches, tags, labels, assignees, teams, check
/// suites, and reactions, each kept for `gh/validate.py` against the pinned description. The data is uneven on
/// purpose: a branch whose name sorts before `main` and is protected by a rule on `main` only, an annotated tag whose
/// object is not its commit, labels whose case-folded order differs from their byte order, suites from two Apps on
/// one commit and a run whose suite has no row, and a comment on another issue.
#[test]
fn the_web_apps_chrome_reads_render_ghes_shapes() {
    use graftyard_api::rest::{
        RepoTeam, list_assignees, list_branches, list_check_suites_for_ref, list_labels, list_reactions, list_tags,
        list_teams,
    };
    use graftyard_identity::RepoPermission as P;
    let mut c = Chain::new(1);
    c.push("refs/heads/main", oid(1));
    c.push("refs/heads/Feature", oid(2));
    c.push("refs/heads/zed", oid(3));
    c.push("refs/tags/v1", oid(7));
    c.push("refs/tags/v2", oid(3));
    c.commit(OWNER, IssueOp::Settings(settings())).unwrap();
    c.commit(OWNER, protect(&["ci"])).unwrap();
    for (name, color) in [("bug", [1, 2, 3]), ("Alpha", [4, 5, 6]), ("beta", [7, 8, 9])] {
        c.commit(OWNER, IssueOp::DefineLabel { id: None, name: LabelName(name.into()), color, description: None })
            .unwrap();
    }
    c.commit(
        READER,
        IssueOp::OpenIssue { title: "t".into(), body: "b".into(), labels: SmallVec::new(), assignees: SmallVec::new() },
    )
    .unwrap();
    let comment = match c
        .commit(READER, IssueOp::Comment { on: CommentTarget::Issue(IssueNumber(1)), body: "c".into(), reply_to: None })
        .unwrap()
    {
        IssueOut::Id(id) => id.0,
        other => panic!("{other:?}"),
    };
    // Two suites on commit 3: App 5's suite 1 (row and two runs), and App 6's run 50 whose suite 9 has no row, as a
    // `POST /check-runs` run makes. Commit 1 has a run of its own, which must not be counted on commit 3.
    let mut other_app = run(50, 3, "lint", None);
    if let IssueOp::RecordCheck(u) = &mut other_app {
        u.app = AppId(DbId(6));
        u.suite = CheckSuiteId(DbId(9));
    }
    c.commit(WRITER, run(40, 3, "build", Some(CheckConclusion::Success))).unwrap();
    c.commit(WRITER, run(41, 3, "test", Some(CheckConclusion::Failure))).unwrap();
    c.commit(WRITER, other_app).unwrap();
    c.commit(WRITER, run(42, 1, "build", Some(CheckConclusion::Success))).unwrap();
    c.entry(
        EntryBody::Events(vec![graftyard_git::repo::RepoEvent::CheckSuite(graftyard_git::issues::CheckSuiteUpdate {
            suite: CheckSuiteId(DbId(1)),
            app: AppId(DbId(5)),
            head_sha: oid(3),
            head_branch: Some(name("refs/heads/zed")),
            version: 1,
            status: CheckStatus::Completed,
            conclusion: Some(CheckConclusion::Failure),
        })]),
        TierSet::METADATA,
    );

    let (h, dir) = (host(), Dir::new(WRITER));
    let r = c.repo(0, &h, &dir);

    let branches = list_branches(&r, "");
    let got: Vec<(&str, bool, &str)> = branches
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["name"].as_str().unwrap(), b["protected"].as_bool().unwrap(), b["commit"]["sha"].as_str().unwrap()))
        .collect();
    let hex = |n: u8| oid(n).hex();
    assert_eq!(got, [("Feature", false, &*hex(2)), ("main", true, &*hex(1)), ("zed", false, &*hex(3))]);
    keep_rest("web-branches", "/repos/{owner}/{repo}/branches", &branches);
    let only = |q: &str| -> Vec<String> {
        list_branches(&r, q).value().as_array().unwrap().iter().map(|b| b["name"].as_str().unwrap().into()).collect()
    };
    assert_eq!(only("protected=true"), ["main"]);
    assert_eq!(only("protected=false"), ["Feature", "zed"]);
    let paged = list_branches(&r, "per_page=2&page=1");
    assert_eq!(paged.value().as_array().unwrap().len(), 2);
    assert!(next(&paged).is_some_and(|l| l.contains("page=2")), "{:?}", paged.header("link"));

    // v1 is annotated: its ref names the tag object 7, which peels to commit 1. v2 is lightweight.
    let peeled: std::collections::BTreeMap<String, graftyard_git::refs::ObjectId> =
        [("v1".to_owned(), oid(1))].into_iter().collect();
    let tags = list_tags(&r, "", &peeled);
    let got: Vec<(&str, &str)> = tags
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["commit"]["sha"].as_str().unwrap()))
        .collect();
    assert_eq!(got, [("v2", &*hex(3)), ("v1", &*hex(1))]);
    keep_rest("web-tags", "/repos/{owner}/{repo}/tags", &tags);

    let labels = list_labels(&r, "");
    let got: Vec<&str> = labels.value().as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(got, ["Alpha", "beta", "bug"]);
    keep_rest("web-labels", "/repos/{owner}/{repo}/labels", &labels);

    // A reader cannot be assigned; the owner and a writer can, in login order, each once.
    let (o, w, rd) = (AccountId(DbId(OWNER)), AccountId(DbId(WRITER)), AccountId(DbId(READER)));
    let people = [(rd, Some(P::Read)), (w, Some(P::Write)), (o, Some(P::Admin)), (w, Some(P::Write))];
    let assignees = list_assignees(&r, &people, "");
    let got: Vec<&str> = assignees.value().as_array().unwrap().iter().map(|u| u["login"].as_str().unwrap()).collect();
    assert_eq!(got, ["alice", "o"]);
    keep_rest("web-assignees", "/repos/{owner}/{repo}/assignees", &assignees);

    let team = |id, name: &str, permission| RepoTeam { id, name: name.into(), slug: name.to_lowercase(), permission };
    let teams = list_teams(&r, vec![team(12, "ops", P::Maintain), team(11, "Core", P::Write)], "");
    let got: Vec<(&str, &str, bool, bool)> = teams
        .value()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            let p = &t["permissions"];
            (t["slug"].as_str().unwrap(), t["permission"].as_str().unwrap(), p["push"] == true, p["admin"] == true)
        })
        .collect();
    assert_eq!(got, [("core", "push", true, false), ("ops", "maintain", true, false)]);
    keep_rest("web-teams", "/repos/{owner}/{repo}/teams", &teams);
    assert_eq!(list_teams(&r, Vec::new(), "").value(), &json!([]));

    // Commit 3's raw object, as the edge reads it from the git cache for `head_commit`.
    let raw = format!(
        "tree {}\nparent {}\nauthor A U <a@example.com> 1700000000 +0000\ncommitter C M <c@example.com> 1700000100 +0000\n\nthe subject\n",
        hex(9),
        hex(1)
    );
    let suites_of = |q: &str| list_check_suites_for_ref(&r, oid(3), raw.as_bytes(), q);
    let suites = suites_of("");
    let v = suites.value();
    let got: Vec<(i64, &str, Option<&str>, u64)> = v["check_suites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let runs = s["latest_check_runs_count"].as_u64().unwrap();
            (s["id"].as_i64().unwrap(), s["status"].as_str().unwrap(), s["head_branch"].as_str(), runs)
        })
        .collect();
    assert_eq!(v["total_count"], 2);
    let head = &v["check_suites"][1]["head_commit"];
    assert_eq!((head["id"].as_str(), head["tree_id"].as_str()), (Some(&*hex(3)), Some(&*hex(9))));
    assert_eq!((&head["message"], &head["author"]["email"]), (&json!("the subject"), &json!("a@example.com")));
    assert_eq!(head["committer"]["name"], "C M");
    assert_eq!(got, [(9, "in_progress", None, 1), (1, "completed", Some("zed"), 2)]);
    keep_rest("web-check-suites", "/repos/{owner}/{repo}/commits/{ref}/check-suites", &suites);
    let ids = |q: &str| -> Vec<i64> {
        let reply = suites_of(q);
        reply.value()["check_suites"].as_array().unwrap().iter().map(|s| s["id"].as_i64().unwrap()).collect()
    };
    assert_eq!(ids("app_id=6"), [9]);
    assert_eq!(ids("check_name=test"), [1]);
    assert_eq!(ids("check_name=nope"), Vec::<i64>::new());
    assert_eq!(list_check_suites_for_ref(&r, oid(1), raw.as_bytes(), "").value()["total_count"], 1, "commit 1's run");

    let on_issue = list_reactions(&r, &["issues", "1"], "");
    assert_eq!((on_issue.status, on_issue.value()), (200, &json!([])));
    keep_rest("web-issue-reactions", "/repos/{owner}/{repo}/issues/{issue_number}/reactions", &on_issue);
    let id = comment.to_string();
    let on_comment = list_reactions(&r, &["issues", "comments", &id], "");
    assert_eq!(on_comment.status, 200);
    keep_rest("web-comment-reactions", "/repos/{owner}/{repo}/issues/comments/{comment_id}/reactions", &on_comment);
    assert_eq!(list_reactions(&r, &["issues", "9"], "").status, 404);
    assert_eq!(list_reactions(&r, &["issues", "comments", "424242"], "").status, 404);
}
