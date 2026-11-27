//! `POST /api/graphql` for the operations gh's `pr` and `issue` commands send (`gh/*.json`).
//!
//! A query runs on one repository's tiers: `route` names the repository from the operation, the edge reads that
//! repository's tiers through `fresh` and calls `query`. A mutation is two calls around a commit: `plan` turns it into
//! the repository command, the edge commits that command, and `render` builds the payload from the command's
//! `&Acked` and a fresh read after it, so no success payload exists before the quorum (`design/DESIGN.md`, Shape,
//! "Invariants").

use graftyard_git::issue_ops::IssueOp;
use graftyard_git::issues::{CommentTarget, IssueNumber, LabelId, MergeMethod, ReviewState};
use graftyard_git::merge::{Merge, MergeError, MergeOut, MergePlan, MergeRequest};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_identity::Principal;
use graftyard_raft::Acked;
use graftyard_types::{AccountId, DbId};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

use crate::exec::{Args, GqlError, Obj, Out, Request, leaf, obj};
use crate::gql::{Node, Repository, search};
use crate::view::Repo;
use crate::wire::{NodeId, NodeType};

/// Which repository an operation addresses: by owner and name, or by the repository id inside a node id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Name { owner: String, name: String },
    Id(i64),
}

fn str_arg<'a>(a: &'a Args, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

/// Finds the repository an operation reads or writes, from its root fields: `repository(owner:, name:)`, a
/// `search` query's `repo:` qualifier, or the repository id that leads a node id (`research/wire-census.md` #27).
/// `None` for an operation that names none, such as gh's `__type` probes, which need no repository.
pub fn route(req: &Request<'_>) -> Result<Option<Target>, GqlError> {
    for (name, a) in req.roots()? {
        let t = match name.as_str() {
            "repository" => match (str_arg(&a, "owner"), str_arg(&a, "name")) {
                (Some(o), Some(n)) => Some(Target::Name { owner: o.into(), name: n.into() }),
                _ => None,
            },
            "search" => str_arg(&a, "query").and_then(|q| {
                let v = q.split_whitespace().find_map(|t| t.strip_prefix("repo:"))?;
                let (o, n) = v.split_once('/')?;
                Some(Target::Name { owner: o.into(), name: n.into() })
            }),
            "node" => str_arg(&a, "id").and_then(route_id),
            // GHES names `dequeuePullRequest`'s pull request `id`, and `updateIssue`'s issue.
            "dequeuePullRequest" | "updateIssue" => {
                a.get("input").and_then(|i| i.get("id")).and_then(Value::as_str).and_then(route_id)
            }
            _ => a.get("input").and_then(|i| {
                ["repositoryId", "subjectId", "pullRequestId", "labelableId", "assignableId"]
                    .iter()
                    .find_map(|k| i.get(*k).and_then(Value::as_str))
                    .and_then(route_id)
            }),
        };
        if t.is_some() {
            return Ok(t);
        }
    }
    Ok(None)
}

/// The repository a node id routes to. A legacy repository-scoped id names none, so it routes nowhere.
fn route_id(id: &str) -> Option<Target> {
    NodeId::parse(id)?.repo().map(|r| Target::Id(r.0.0))
}

/// The `Query` root over one repository, or over none for a probe.
struct QueryRoot<'r> {
    r: Option<&'r Repo<'r>>,
}

impl Obj for QueryRoot<'_> {
    fn type_name(&self) -> &'static str {
        "Query"
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        let r = self.r?;
        Some(match f {
            "repository" => {
                let (o, n) = (str_arg(a, "owner").unwrap_or(""), str_arg(a, "name").unwrap_or(""));
                if o.eq_ignore_ascii_case(&r.at.owner) && n.eq_ignore_ascii_case(&r.at.name) {
                    obj(Repository { r, at: r.at.clone() })
                } else {
                    Out::Err("NOT_FOUND", format!("Could not resolve to a Repository with the name '{o}/{n}'."))
                }
            }
            "node" => match str_arg(a, "id").and_then(|id| item_of(r, id)) {
                Some(n) => obj(Node { r, it: r.item(n)?, listed: false }),
                None => Out::Err(
                    "NOT_FOUND",
                    format!("Could not resolve to a node with the global id of '{}'", {
                        str_arg(a, "id").unwrap_or("")
                    }),
                ),
            },
            "search" => search(r, a),
            _ => return None,
        })
    }
}

/// The issue or pull request a node id names in this repository, in either id format.
fn item_of(r: &Repo<'_>, id: &str) -> Option<IssueNumber> {
    let id = NodeId::parse(id)?;
    // A legacy id names no repository: it resolves once the request named the repository another way.
    if !matches!(id.ty, NodeType::Issue | NodeType::PullRequest) || !(id.legacy || id.repo() == Some(r.at.id)) {
        return None;
    }
    let it = r.item_by_id(DbId(id.id()))?;
    (it.is_pull() == (id.ty == NodeType::PullRequest)).then_some(it.n)
}

/// The number of the milestone a `MI_` node id names in this repository, which holds the milestone's id, not its number.
fn milestone_of(r: &Repo<'_>, id: &str) -> Option<u32> {
    let id = NodeId::parse(id).filter(|n| n.ty == NodeType::Milestone && n.repo() == Some(r.at.id))?;
    graftyard_git::tiers::milestones(r.meta).into_iter().find(|(_, m)| m.id.0 == id.id()).map(|(n, _)| n)
}

/// The signed-in user, for `viewer`: gh's `auth login` and `auth status` ask `viewer { login }` and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Viewer {
    pub id: graftyard_types::AccountId,
    pub login: String,
}

impl Obj for Viewer {
    fn type_name(&self) -> &'static str {
        "User"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "login" => crate::exec::leaf(self.login.clone()),
            "id" => crate::exec::leaf(NodeId::top(NodeType::User, self.id.0.0).encode()),
            "databaseId" => crate::exec::leaf(graftyard_wire::database_id(self.id.0.0)),
            "name" => Out::Null,
            _ => return None,
        })
    }
}

/// A query that names no repository, with the caller as `viewer`. Any other root field is `undefinedField`.
pub fn query_viewer(req: &Request<'_>, viewer: Option<&Viewer>) -> Value {
    struct Root<'v>(Option<&'v Viewer>);
    impl Obj for Root<'_> {
        fn type_name(&self) -> &'static str {
            "Query"
        }
        fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
            match (f, self.0) {
                ("viewer", Some(v)) => Some(crate::exec::obj(v)),
                _ => None,
            }
        }
    }
    match req.is_mutation() {
        Ok(false) => req.run(&Root(viewer)),
        Ok(true) => json!({ "errors": [{ "type": "unsupported", "message": "a mutation runs through plan" }] }),
        Err(e) => json!({ "errors": [e.json()] }),
    }
}

/// `query`, with the pull requests it rendered `mergeable` for with no current test merge (`crate::gql::MergeAsks`).
pub fn query_asks(req: &Request<'_>, r: Option<&Repo<'_>>) -> (Value, crate::gql::MergeAsks) {
    crate::gql::ASKS.with(|a| *a.borrow_mut() = Some(crate::gql::MergeAsks::default()));
    let v = query(req, r);
    (v, crate::gql::ASKS.with(|a| a.borrow_mut().take()).unwrap_or_default())
}

/// Runs a query. `r` is the repository `route` named, read through `fresh`, or `None` when it named none.
pub fn query(req: &Request<'_>, r: Option<&Repo<'_>>) -> Value {
    match req.is_mutation() {
        Ok(false) => req.run(&QueryRoot { r }),
        Ok(true) => json!({ "errors": [{ "type": "unsupported", "message": "a mutation runs through plan" }] }),
        Err(e) => json!({ "errors": [e.json()] }),
    }
}

// ---- mutations ----

/// What a mutation commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// An `IssueCmd` on the repository chain.
    Issue(Box<IssueOp>),
    /// `mergePullRequest`. The edge asks the git cache for the `MergePlan` of `merge_request(n, method)`, then builds
    /// the `Merge` command with `merge_command` and commits it.
    Merge(PlannedMerge),
    /// `enqueuePullRequest`, and `enablePullRequestAutoMerge` on a base with a merge queue, which is how gh adds a
    /// pull request to the queue (`gh pr merge` on such a base): a `QueueOp::Enqueue` at the head the edge read.
    Enqueue { n: IssueNumber, head: ObjectId },
    /// `dequeuePullRequest`.
    Dequeue { n: IssueNumber },
}

/// The inputs of a planned `mergePullRequest`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlannedMerge {
    pub n: IssueNumber,
    pub method: MergeMethod,
    pub expected_head: Option<ObjectId>,
}

/// A planned mutation: its root field, the command, and the issue it is about, for the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub field: String,
    pub plan: Plan,
    pub subject: Option<IssueNumber>,
}

fn unprocessable(field: &str, message: impl Into<String>) -> Value {
    json!({ "data": { field: null }, "errors": [{ "type": "UNPROCESSABLE", "path": [field], "message": message.into() }] })
}

fn not_found(field: &str, message: impl Into<String>) -> Value {
    json!({ "data": { field: null }, "errors": [{ "type": "NOT_FOUND", "path": [field], "message": message.into() }] })
}

/// Turns a mutation into the command the edge commits, reading the fresh view `r` for the ids and refs it names. The
/// error is the whole GraphQL reply.
pub fn plan(req: &Request<'_>, r: &Repo<'_>) -> Result<Planned, Value> {
    let err = |e: GqlError| json!({ "errors": [e.json()] });
    if !req.is_mutation().map_err(err)? {
        return Err(json!({ "errors": [{ "type": "unsupported", "message": "a query runs through query" }] }));
    }
    let roots = req.roots().map_err(err)?;
    let [(field, args)] = roots.as_slice() else {
        return Err(json!({ "errors": [{ "type": "unsupported", "message": "one mutation per request" }] }));
    };
    let input = args.get("input").and_then(Value::as_object).cloned().unwrap_or_default();
    let s = |k: &str| input.get(k).and_then(Value::as_str);
    let repo_ok = |k: &str| {
        s(k).and_then(NodeId::parse).is_some_and(|n| n.ty == NodeType::Repository && n.repo() == Some(r.at.id))
    };
    let planned = |plan, subject| Ok(Planned { field: field.clone(), plan, subject });
    // F149: once 32.37 is enabled, `milestoneId` (a milestone's node id, or `null` to clear it) rides the command the
    // rest of the input plans, as REST's `milestone` does (`crate::rest::with_milestone`); until then it is refused as
    // not served, as before.
    let milestone = || -> Result<Option<Option<u32>>, Value> {
        Ok(match input.get("milestoneId") {
            None => None,
            Some(_) if !r.dir.milestones() => None,
            Some(Value::Null) => Some(None),
            Some(v) => {
                let id = v.as_str().unwrap_or("");
                let n = milestone_of(r, id).ok_or_else(|| {
                    not_found(field, format!("Could not resolve to a node with the global id of '{id}'."))
                })?;
                Some(Some(n))
            }
        })
    };
    let served = |k: &str| k == "milestoneId" && r.dir.milestones();
    match field.as_str() {
        "createIssue" => {
            if !repo_ok("repositoryId") {
                return Err(not_found(field, "Could not resolve to a Repository with that id."));
            }
            for k in ["milestoneId", "issueTypeId", "parentIssueId", "issueTemplate", "agentAssignment"] {
                if served(k) {
                    continue;
                }
                if input.get(k).is_some_and(|v| !v.is_null()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            for k in ["projectIds", "projectV2Ids", "issueFields"] {
                if input.get(k).and_then(Value::as_array).is_some_and(|v| !v.is_empty()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            let labels = ids(&input, "labelIds", NodeType::Label, r).map_err(|m| not_found(field, m))?;
            let assignees = ids(&input, "assigneeIds", NodeType::User, r).map_err(|m| not_found(field, m))?;
            let op = IssueOp::OpenIssue {
                title: s("title").unwrap_or("").into(),
                body: s("body").unwrap_or("").into(),
                labels: labels.into_iter().map(|i| LabelId(DbId(i))).collect(),
                assignees: assignees.into_iter().map(|i| AccountId(DbId(i))).collect::<SmallVec<_>>(),
            };
            let op = match milestone()? {
                Some(m) => crate::rest::with_milestone(op, m),
                None => op,
            };
            planned(Plan::Issue(Box::new(op)), None)
        }
        "createPullRequest" => {
            if !repo_ok("repositoryId") {
                return Err(not_found(field, "Could not resolve to a Repository with that id."));
            }
            let head_repo = match s("headRepositoryId") {
                None => r.at.clone(),
                Some(id) => match NodeId::parse(id).filter(|n| n.ty == NodeType::Repository) {
                    Some(n) => r
                        .dir
                        .repo(graftyard_types::RepoId(DbId(n.id())))
                        .ok_or_else(|| not_found(field, "Could not resolve to a Repository with that id."))?,
                    _ => return Err(not_found(field, "Could not resolve to a Repository with that id.")),
                },
            };
            let base_name = s("baseRefName").unwrap_or("");
            let head_name = s("headRefName").unwrap_or("");
            let branch = |b: &str| RefName::parse(format!("refs/heads/{b}").as_bytes()).ok();
            let (Some(base), Some(head)) = (branch(base_name), branch(head_name)) else {
                return Err(unprocessable(field, "Head ref must be a branch"));
            };
            let base_sha = r.ref_value(&base);
            let head_sha = if head_repo.id == r.at.id { r.ref_value(&head) } else { r.dir.ref_of(head_repo.id, &head) };
            let (Some(base_sha), Some(head_sha)) = (base_sha, head_sha) else {
                return Err(unprocessable(field, "Head sha can't be blank, Base sha can't be blank"));
            };
            let op = IssueOp::OpenPr {
                head: graftyard_git::issues::HeadSpec { repo: head_repo.id, branch: head },
                base,
                title: s("title").unwrap_or("").into(),
                body: s("body").unwrap_or("").into(),
                draft: input.get("draft").and_then(Value::as_bool).unwrap_or(false),
                head_sha,
                base_sha,
            };
            planned(Plan::Issue(Box::new(op)), None)
        }
        "addComment" => {
            let n = s("subjectId")
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let op =
                IssueOp::Comment { on: CommentTarget::Issue(n), body: s("body").unwrap_or("").into(), reply_to: None };
            planned(Plan::Issue(Box::new(op)), Some(n))
        }
        // `gh pr edit --body`/`--title` (AM1's plan blocks): `updatePullRequest` with `body` or `title`, one change per
        // command; `gh issue edit`'s `updateIssue` likewise, served once 32.37 is enabled (until then it is
        // `undefinedField`, and the edge does not route it, as on main). Once 32.37 is enabled, a `milestoneId` (`gh pr edit
        // --milestone`, `gh issue edit --milestone`, and `gh pr create --milestone`'s second mutation) rides the same
        // command: alone it is a `SetMilestone`, with a title or body an `EditWithMilestone`, as REST's issue `PATCH`
        // plans it. Every other field is not served, and a request naming one is refused, so nothing is half applied.
        "updatePullRequest" | "updateIssue" if field == "updatePullRequest" || r.dir.milestones() => {
            let pull = field == "updatePullRequest";
            let n = s(if pull { "pullRequestId" } else { "id" })
                .and_then(|id| item_of(r, id))
                .filter(|n| r.item(*n).is_some_and(|it| it.is_pull() == pull))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let unserved: &[&str] = if pull {
                &["assigneeIds", "baseRefName", "labelIds", "maintainerCanModify", "milestoneId", "projectIds", "state"]
            } else {
                &["assigneeIds", "labelIds", "milestoneId", "projectIds", "state", "issueTypeId", "agentAssignment"]
            };
            for k in unserved {
                if !served(k) && input.get(*k).is_some_and(|v| !v.is_null()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            use graftyard_git::issues::IssueChange;
            let mut changes: SmallVec<[IssueChange; 3]> = SmallVec::new();
            changes.extend(s("title").map(|t| IssueChange::Title(t.into())));
            changes.extend(s("body").map(|b| IssueChange::Body(b.into())));
            let op = match milestone()? {
                Some(m) => crate::rest::with_milestone(IssueOp::Edit { n, changes, labels: None, assignees: None }, m),
                None if changes.len() == 1 => IssueOp::Change { n, change: changes.remove(0) },
                // `gh issue edit -t -b` sends both: one `Edit`, all or nothing, as REST's `PATCH` plans it.
                None if !pull && !changes.is_empty() => IssueOp::Edit { n, changes, labels: None, assignees: None },
                None => return Err(unprocessable(field, "exactly one of body and title is served per request")),
            };
            planned(Plan::Issue(Box::new(op)), Some(n))
        }
        // `gh pr ready` and `gh pr ready --undo` (B0-36's ready-for-review): the draft flag, which `decide` refuses an
        // `Agent` to clear (`AgentRule::ReadyForReview`).
        "markPullRequestReadyForReview" | "convertPullRequestToDraft" => {
            let n = s("pullRequestId")
                .and_then(|id| item_of(r, id))
                .filter(|n| r.item(*n).is_some_and(|it| it.is_pull()))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let draft = field == "convertPullRequestToDraft";
            planned(
                Plan::Issue(Box::new(IssueOp::Change { n, change: graftyard_git::issues::IssueChange::Draft(draft) })),
                Some(n),
            )
        }
        // `gh issue edit --add-label` and `--remove-label` (B0-36's labels): `ApplyLabels`, which `decide` refuses an
        // `Agent` for a label outside the repository's allowlist.
        "addLabelsToLabelable" | "removeLabelsFromLabelable" => {
            let n = s("labelableId")
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let labels: SmallVec<[LabelId; 4]> = ids(&input, "labelIds", NodeType::Label, r)
                .map_err(|m| not_found(field, m))?
                .into_iter()
                .map(|i| LabelId(DbId(i)))
                .collect();
            if labels.is_empty() {
                return Err(unprocessable(field, "labelIds can't be blank"));
            }
            let (add, remove) =
                if field == "addLabelsToLabelable" { (labels, SmallVec::new()) } else { (SmallVec::new(), labels) };
            planned(Plan::Issue(Box::new(IssueOp::ApplyLabels { n, add, remove })), Some(n))
        }
        // F20's assignment changes: `Assign`, whose `decide` drops an assignee without push access, as REST does.
        // `assigneeIds` are `User` node ids, or the `BOT_` id of `graftyard-agent`'s bot account (`AppId::bot`).
        "addAssigneesToAssignable" | "removeAssigneesFromAssignable" => {
            if input.get("agentAssignment").is_some_and(|v| !v.is_null()) {
                return Err(unprocessable(field, "agentAssignment is not served"));
            }
            let n = s("assignableId")
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let mut who: SmallVec<[AccountId; 2]> = SmallVec::new();
            for v in input.get("assigneeIds").and_then(Value::as_array).into_iter().flatten() {
                let id = v.as_str().unwrap_or("");
                let a = match NodeId::parse(id) {
                    Some(x) if x.ty == NodeType::User => AccountId(DbId(x.id())),
                    Some(x) if x.ty == NodeType::Bot => graftyard_identity::AppId(DbId(x.id())).bot(),
                    _ => {
                        return Err(not_found(
                            field,
                            format!("Could not resolve to a node with the global id of '{id}'."),
                        ));
                    }
                };
                if !who.contains(&a) {
                    who.push(a);
                }
            }
            if who.is_empty() {
                return Err(unprocessable(field, "assigneeIds can't be blank"));
            }
            let (add, remove) =
                if field == "addAssigneesToAssignable" { (who, SmallVec::new()) } else { (SmallVec::new(), who) };
            planned(Plan::Issue(Box::new(IssueOp::Assign { n, add, remove })), Some(n))
        }
        // `gh pr review`: `PullRequestReviewAdd` with `event` and `body`, a review at the pull request's head.
        "addPullRequestReview" => {
            let n = s("pullRequestId")
                .filter(|id| id.starts_with("PR_"))
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            for k in ["comments", "threads"] {
                if input.get(k).and_then(Value::as_array).is_some_and(|v| !v.is_empty()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            let state = match s("event") {
                Some("APPROVE") => ReviewState::Approved,
                Some("REQUEST_CHANGES") => ReviewState::ChangesRequested,
                Some("COMMENT") => ReviewState::Commented,
                // GHES leaves a review with no event pending; a pending review is not served.
                _ => return Err(unprocessable(field, "a pending review is not served")),
            };
            let it = r.item(n).ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let (_, head) = it.pull.as_ref().ok_or_else(|| not_found(field, "Not a pull request"))?;
            let commit = match s("commitOID") {
                Some(h) => ObjectId::parse_hex(h.as_bytes()).ok_or_else(|| unprocessable(field, "commitOID"))?,
                None => head.head_sha,
            };
            let op = IssueOp::Review { n, state, commit, body: s("body").unwrap_or("").into() };
            planned(Plan::Issue(Box::new(op)), Some(n))
        }
        "mergePullRequest" => {
            let n = s("pullRequestId")
                .filter(|id| id.starts_with("PR_"))
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let method = match s("mergeMethod").unwrap_or("MERGE") {
                "MERGE" => MergeMethod::Merge,
                "SQUASH" => MergeMethod::Squash,
                "REBASE" => MergeMethod::Rebase,
                m => return Err(unprocessable(field, format!("unknown merge method {m}"))),
            };
            if let Some(s) = r.settings()
                && !s.merge_methods.contains(&method)
            {
                return Err(unprocessable(field, "Merge method is not allowed on this repository"));
            }
            let expected_head = match s("expectedHeadOid") {
                Some(h) => {
                    Some(ObjectId::parse_hex(h.as_bytes()).ok_or_else(|| unprocessable(field, "expectedHeadOid"))?)
                }
                None => None,
            };
            planned(Plan::Merge(PlannedMerge { n, method, expected_head }), Some(n))
        }
        "enqueuePullRequest" | "enablePullRequestAutoMerge" | "dequeuePullRequest" => {
            // GHES names `dequeuePullRequest`'s pull request `id`, the other two `pullRequestId`.
            let n = s(if field == "dequeuePullRequest" { "id" } else { "pullRequestId" })
                .filter(|id| id.starts_with("PR_"))
                .and_then(|id| item_of(r, id))
                .ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let it = r.item(n).ok_or_else(|| not_found(field, "Could not resolve to a node with that global id."))?;
            let (pull, head) = it.pull.as_ref().ok_or_else(|| not_found(field, "Not a pull request"))?;
            if field == "dequeuePullRequest" {
                return planned(Plan::Dequeue { n }, Some(n));
            }
            // Auto-merge without a queue is not built: GHES's `allow_auto_merge` setting is off unless enabled.
            if !r.protections(&pull.base).iter().any(|p| p.merge_queue.is_some()) {
                return Err(unprocessable(field, "Auto merge is not allowed for this repository"));
            }
            let expected = match s("expectedHeadOid") {
                Some(h) => {
                    Some(ObjectId::parse_hex(h.as_bytes()).ok_or_else(|| unprocessable(field, "expectedHeadOid"))?)
                }
                None => None,
            };
            if expected.is_some_and(|h| h != head.head_sha) {
                return Err(unprocessable(field, MergeError::HeadModified.message()));
            }
            planned(Plan::Enqueue { n, head: head.head_sha }, Some(n))
        }
        f => Err(json!({ "errors": [{ "type": "undefinedField", "path": [f],
            "message": format!("Field '{f}' doesn't exist on type 'Mutation'") }] })),
    }
}

/// Node ids of `ty` in `input[key]`: a repository-scoped type must name this repository; an account type names one id.
fn ids(input: &Map<String, Value>, key: &str, ty: NodeType, r: &Repo<'_>) -> Result<Vec<i64>, String> {
    let Some(list) = input.get(key).and_then(Value::as_array) else { return Ok(Vec::new()) };
    list.iter()
        .map(|v| {
            let id = v.as_str().unwrap_or("");
            match NodeId::parse(id) {
                Some(n) if n.ty == ty && (!ty.repo_scoped() || n.repo() == Some(r.at.id)) => Ok(n.id()),
                _ => Err(format!("Could not resolve to a node with the global id of '{id}'.")),
            }
        })
        .collect()
}

/// The GraphQL reply to a committed command's refusal, with GitHub's text where the design gives one.
pub fn refused(p: &Planned, e: &graftyard_git::issue_ops::IssueErr) -> Value {
    refused_field(&p.field, e)
}

/// `refused` from the mutation's root field alone, as the member that refused the command renders it.
pub fn refused_field(f: &str, e: &graftyard_git::issue_ops::IssueErr) -> Value {
    use graftyard_git::issue_ops::IssueErr as E;
    match e {
        E::Unauthorized(_) => json!({ "message": "Bad credentials", "status": "401" }),
        E::Forbidden => json!({ "data": { f: null }, "errors": [{ "type": "FORBIDDEN", "path": [f],
            "message": "Resource not accessible by integration" }] }),
        E::NotFound | E::NotPull => not_found(f, "Could not resolve to a node with that global id."),
        E::Exists => unprocessable(f, "A pull request already exists for this head and base."),
        E::HeadMissing | E::BaseMissing | E::HeadMoved => {
            unprocessable(f, "Head sha can't be blank, Base sha can't be blank")
        }
        E::Invalid(why) => unprocessable(f, *why),
        E::UnknownLabel(_) => not_found(f, "Could not resolve to a Label with that id."),
        E::NotOpen => unprocessable(f, "Pull request is not open"),
        E::OwnApproval => unprocessable(f, "Can not approve your own pull request"),
        E::OwnerChange => unprocessable(f, "owner"),
        E::OrgMember => unprocessable(f, "User is a member of the organization"),
        // The host leases a block and re-sends; a client never sees this.
        E::NeedIdBlock | E::Unavailable => json!({ "message": "Service Unavailable", "status": "503" }),
        // An agent rule: GraphQL's FORBIDDEN, with a text that names the rule (AM1-WP2).
        E::Agent(a) => json!({ "data": { f: null }, "errors": [{ "type": "FORBIDDEN", "path": [f],
            "message": &*a.text }] }),
        // Task commands are not GraphQL mutations; a refusal of one never reaches here.
        E::Task(_) => unprocessable(f, "task"),
        // Nor are lease commands.
        E::Lease(_) => unprocessable(f, "lease"),
    }
}

/// The payload of a committed `Plan::Issue`, from its `&Acked` and `after`, a fresh read at or after its index.
pub fn render(
    req: &Request<'_>,
    p: &Planned,
    acked: &Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
) -> Value {
    render_payload(req, &p.field, p.subject, acked, after)
}

/// `render` from what the payload reads of the plan: its root field and its issue. The leader that committed the
/// command renders with this, since only it holds the `&Acked`.
pub fn render_payload(
    req: &Request<'_>,
    field: &str,
    subject: Option<IssueNumber>,
    acked: &Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
) -> Value {
    req.run(&MutationRoot { p: &Shown { field: field.to_owned(), subject }, out: &acked.out, r: after })
}

/// The part of a `Planned` its payload reads.
struct Shown {
    field: String,
    subject: Option<IssueNumber>,
}

struct MutationRoot<'a> {
    p: &'a Shown,
    out: &'a graftyard_git::issue_ops::IssueOut,
    r: &'a Repo<'a>,
}

impl Obj for MutationRoot<'_> {
    fn type_name(&self) -> &'static str {
        "Mutation"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == self.p.field).then(|| obj(Payload { root: self }))
    }
}

struct Payload<'a> {
    root: &'a MutationRoot<'a>,
}

impl Obj for Payload<'_> {
    fn type_name(&self) -> &'static str {
        match self.root.p.field.as_str() {
            "createIssue" => "CreateIssuePayload",
            "createPullRequest" => "CreatePullRequestPayload",
            "addPullRequestReview" => "AddPullRequestReviewPayload",
            "updatePullRequest" => "UpdatePullRequestPayload",
            "updateIssue" => "UpdateIssuePayload",
            "markPullRequestReadyForReview" => "MarkPullRequestReadyForReviewPayload",
            "convertPullRequestToDraft" => "ConvertPullRequestToDraftPayload",
            "addLabelsToLabelable" => "AddLabelsToLabelablePayload",
            "removeLabelsFromLabelable" => "RemoveLabelsFromLabelablePayload",
            "addAssigneesToAssignable" => "AddAssigneesToAssignablePayload",
            "removeAssigneesFromAssignable" => "RemoveAssigneesFromAssignablePayload",
            _ => "AddCommentPayload",
        }
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        use graftyard_git::issue_ops::IssueOut;
        let r = self.root.r;
        let node = |n: IssueNumber| r.item(n).map(|it| obj(Node { r, it, listed: false })).unwrap_or(Out::Null);
        Some(match (f, self.root.out) {
            ("clientMutationId", _) => leaf(Value::Null),
            ("issue" | "pullRequest", IssueOut::Number(n)) => node(*n),
            ("issue", IssueOut::Done) if self.root.p.field == "updateIssue" => node(self.root.p.subject?),
            ("pullRequest", IssueOut::Done)
                if matches!(
                    self.root.p.field.as_str(),
                    "updatePullRequest" | "markPullRequestReadyForReview" | "convertPullRequestToDraft"
                ) =>
            {
                node(self.root.p.subject?)
            }
            ("labelable", IssueOut::Done)
                if matches!(self.root.p.field.as_str(), "addLabelsToLabelable" | "removeLabelsFromLabelable") =>
            {
                node(self.root.p.subject?)
            }
            ("assignable", IssueOut::Done)
                if matches!(
                    self.root.p.field.as_str(),
                    "addAssigneesToAssignable" | "removeAssigneesFromAssignable"
                ) =>
            {
                node(self.root.p.subject?)
            }
            ("actor", _) if matches!(self.root.p.field.as_str(), "updatePullRequest" | "updateIssue") => Out::Null,
            ("commentEdge", IssueOut::Id(id)) => obj(Edge { r, n: self.root.p.subject?, id: *id }),
            ("pullRequestReview", IssueOut::Id(id)) => obj(crate::gql::review(r, self.root.p.subject?, *id)?),
            ("subject", _) => node(self.root.p.subject?),
            _ => return None,
        })
    }
}

struct Edge<'a> {
    r: &'a Repo<'a>,
    n: IssueNumber,
    id: DbId,
}

impl Obj for Edge<'_> {
    fn type_name(&self) -> &'static str {
        "IssueCommentEdge"
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        if f != "node" {
            return None;
        }
        let _ = a;
        let it = self.r.item(self.n)?;
        let (id, row) = self.r.comments_of(self.n).into_iter().find(|(id, _)| id.0 == self.id)?;
        Some(obj(crate::gql::comment(self.r, id.0, row, it.is_pull(), self.n)))
    }
}

/// What the edge asks the git cache to merge for a planned `mergePullRequest`, read from the fresh core tier: the
/// base's tip and the pull request's head. A pull request that is closed, merged, or a draft is refused here, with
/// the reply `refused_merge_field` renders for the mutation's root `field`.
pub fn merge_request(field: &str, m: &PlannedMerge, r: &Repo<'_>) -> Result<MergeRequest, Value> {
    graftyard_git::merge::merge_request(r.core, m.n, m.method).map_err(|e| refused_merge_field(field, &e))
}

/// The `Merge` command for a planned `mergePullRequest`, from the git cache's plan. Its `decide` re-checks the head,
/// the base, and every rule on the fold it commits on.
pub fn merge_command(m: &PlannedMerge, plan: MergePlan, actor: Principal) -> Merge {
    Merge { n: m.n, plan, expected_head: m.expected_head, actor }
}

/// The GraphQL reply to a refused merge, with GitHub's text. A 409 is never retried into success.
pub fn refused_merge(p: &Planned, e: &MergeError) -> Value {
    refused_merge_field(&p.field, e)
}

/// `refused_merge` from the mutation's root field alone, as the member that refused the command renders it.
pub fn refused_merge_field(f: &str, e: &MergeError) -> Value {
    match e {
        MergeError::Unauthorized(_) => json!({ "message": "Bad credentials", "status": "401" }),
        MergeError::Forbidden | MergeError::Agent(_) => json!({ "data": { f: null }, "errors": [{ "type": "FORBIDDEN",
            "path": [f], "message": e.message() }] }),
        MergeError::NotFound => not_found(f, e.message()),
        MergeError::Store(_) => json!({ "message": "Service Unavailable", "status": "503" }),
        _ => unprocessable(f, e.message()),
    }
}

/// The payload of a committed `mergePullRequest`, from the `Merge` command's `&Acked`. gh selects only
/// `clientMutationId`.
pub fn render_merge(req: &Request<'_>, _acked: &Acked<MergeOut>) -> Value {
    req.run(&Merged { field: "mergePullRequest", r: None, n: None })
}

/// The payload of a committed merge-queue mutation (`Plan::Enqueue` or `Plan::Dequeue`), from its `&Acked` and
/// `after`, the leader's tiers once the entry applied: `clientMutationId`, the pull request, and the queue entry.
pub fn render_queued(req: &Request<'_>, field: &str, n: IssueNumber, _acked: &Acked<()>, after: &Repo<'_>) -> Value {
    req.run(&Merged { field: payload_field(field), r: Some(after), n: Some(n) })
}

fn payload_field(f: &str) -> &'static str {
    match f {
        "enqueuePullRequest" => "enqueuePullRequest",
        "enablePullRequestAutoMerge" => "enablePullRequestAutoMerge",
        "dequeuePullRequest" => "dequeuePullRequest",
        _ => "mergePullRequest",
    }
}

struct Merged<'a> {
    field: &'static str,
    r: Option<&'a Repo<'a>>,
    n: Option<IssueNumber>,
}

impl Obj for Merged<'_> {
    fn type_name(&self) -> &'static str {
        "Mutation"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == self.field).then(|| obj(MergedPayload { p: self }))
    }
}

struct MergedPayload<'a> {
    p: &'a Merged<'a>,
}

impl Obj for MergedPayload<'_> {
    fn type_name(&self) -> &'static str {
        match self.p.field {
            "enqueuePullRequest" => "EnqueuePullRequestPayload",
            "enablePullRequestAutoMerge" => "EnablePullRequestAutoMergePayload",
            "dequeuePullRequest" => "DequeuePullRequestPayload",
            _ => "MergePullRequestPayload",
        }
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        let _ = a;
        let pr = || match (self.p.r, self.p.n) {
            (Some(r), Some(n)) => r.item(n).map(|it| obj(Node { r, it, listed: false })),
            _ => None,
        };
        match f {
            "clientMutationId" => Some(leaf(Value::Null)),
            "pullRequest" => Some(pr().unwrap_or(Out::Null)),
            // The entry is not rendered: no GHES `MergeQueueEntry` node type is served yet.
            "mergeQueueEntry" | "actor" => Some(Out::Null),
            _ => None,
        }
    }
}
