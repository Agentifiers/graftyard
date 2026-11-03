//! `POST /api/graphql` for the operations gh's `pr` and `issue` commands send (`gh/*.json`).
//!
//! A query runs on one repository's tiers: `route` names the repository from the operation, the edge reads that
//! repository's tiers through `fresh` and calls `query`. A mutation is two calls around a commit: `plan` turns it into
//! the repository command, the edge commits that command, and `render` builds the payload from the command's
//! `&Acked` and a fresh read after it, so no success payload exists before the quorum (`design/DESIGN.md`, Shape,
//! "Invariants").

use graftyard_git::issue_ops::IssueOp;
use graftyard_git::issues::{CommentTarget, IssueNumber, LabelId, MergeMethod};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_raft::Acked;
use graftyard_types::{AccountId, DbId};
use serde_json::{Map, Value, json};
use smallvec::SmallVec;

use crate::exec::{Args, GqlError, Obj, Out, Request, leaf, obj};
use crate::gql::{Node, Repository, search};
use crate::view::Repo;
use crate::wire::parse_node_id;

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
            "node" => str_arg(&a, "id").and_then(|id| Some(Target::Id(*parse_node_id(id)?.1.first()?))),
            _ => a.get("input").and_then(|i| {
                ["repositoryId", "subjectId", "pullRequestId"]
                    .iter()
                    .find_map(|k| i.get(*k).and_then(Value::as_str))
                    .and_then(|id| Some(Target::Id(*parse_node_id(id)?.1.first()?)))
            }),
        };
        if t.is_some() {
            return Ok(t);
        }
    }
    Ok(None)
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
                Some(n) => obj(Node { r, it: r.item(n)? }),
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

/// The issue or pull request a node id names in this repository.
fn item_of(r: &Repo<'_>, id: &str) -> Option<IssueNumber> {
    let (prefix, ids) = parse_node_id(id)?;
    if !matches!(prefix, "I" | "PR") || ids.first() != Some(&r.at.id.0.0) {
        return None;
    }
    let it = r.item_by_id(DbId(*ids.get(1)?))?;
    (it.is_pull() == (prefix == "PR")).then_some(it.n)
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
    /// `mergePullRequest`: the `Merge` command, its `MergePlan`, and `MergeError` are M2-WP3.
    Merge { n: IssueNumber, method: MergeMethod, expected_head: Option<ObjectId> },
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
    let repo_ok = |k: &str| s(k).and_then(parse_node_id).is_some_and(|(p, ids)| p == "R" && ids == [r.at.id.0.0]);
    let planned = |plan, subject| Ok(Planned { field: field.clone(), plan, subject });
    match field.as_str() {
        "createIssue" => {
            if !repo_ok("repositoryId") {
                return Err(not_found(field, "Could not resolve to a Repository with that id."));
            }
            for k in ["milestoneId", "issueTypeId", "parentIssueId", "issueTemplate", "agentAssignment"] {
                if input.get(k).is_some_and(|v| !v.is_null()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            for k in ["projectIds", "projectV2Ids", "issueFields"] {
                if input.get(k).and_then(Value::as_array).is_some_and(|v| !v.is_empty()) {
                    return Err(unprocessable(field, format!("{k} is not served")));
                }
            }
            let labels = ids(&input, "labelIds", "LA", r).map_err(|m| not_found(field, m))?;
            let assignees = ids(&input, "assigneeIds", "U", r).map_err(|m| not_found(field, m))?;
            let op = IssueOp::OpenIssue {
                title: s("title").unwrap_or("").into(),
                body: s("body").unwrap_or("").into(),
                labels: labels.into_iter().map(|i| LabelId(DbId(i))).collect(),
                assignees: assignees.into_iter().map(|i| AccountId(DbId(i))).collect::<SmallVec<_>>(),
            };
            planned(Plan::Issue(Box::new(op)), None)
        }
        "createPullRequest" => {
            if !repo_ok("repositoryId") {
                return Err(not_found(field, "Could not resolve to a Repository with that id."));
            }
            let head_repo = match s("headRepositoryId") {
                None => r.at.clone(),
                Some(id) => match parse_node_id(id) {
                    Some(("R", ids)) if ids.len() == 1 => r
                        .dir
                        .repo(graftyard_types::RepoId(DbId(ids[0])))
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
            planned(Plan::Merge { n, method, expected_head }, Some(n))
        }
        f => Err(json!({ "errors": [{ "type": "undefinedField", "path": [f],
            "message": format!("Field '{f}' doesn't exist on type 'Mutation'") }] })),
    }
}

/// Node ids of `prefix` in `input[key]`, each scoped to this repository (`[repo, id]`) or an account (`[id]`).
fn ids(input: &Map<String, Value>, key: &str, prefix: &str, r: &Repo<'_>) -> Result<Vec<i64>, String> {
    let Some(list) = input.get(key).and_then(Value::as_array) else { return Ok(Vec::new()) };
    list.iter()
        .map(|v| {
            let id = v.as_str().unwrap_or("");
            match parse_node_id(id) {
                Some((p, ids)) if p == prefix && prefix == "LA" && ids.len() == 2 && ids[0] == r.at.id.0.0 => {
                    Ok(ids[1])
                }
                Some((p, ids)) if p == prefix && prefix == "U" && ids.len() == 1 => Ok(ids[0]),
                _ => Err(format!("Could not resolve to a node with the global id of '{id}'.")),
            }
        })
        .collect()
}

/// The GraphQL reply to a committed command's refusal, with GitHub's text where the design gives one.
pub fn refused(p: &Planned, e: &graftyard_git::issue_ops::IssueErr) -> Value {
    use graftyard_git::issue_ops::IssueErr as E;
    let f = &p.field;
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
        // The host leases a block and re-sends; a client never sees this.
        E::NeedIdBlock => json!({ "message": "Service Unavailable", "status": "503" }),
    }
}

/// The payload of a committed `Plan::Issue`, from its `&Acked` and `after`, a fresh read at or after its index.
pub fn render(
    req: &Request<'_>,
    p: &Planned,
    acked: &Acked<graftyard_git::issue_ops::IssueOut>,
    after: &Repo<'_>,
) -> Value {
    req.run(&MutationRoot { p, out: &acked.out, r: after })
}

struct MutationRoot<'a> {
    p: &'a Planned,
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
            _ => "AddCommentPayload",
        }
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        use graftyard_git::issue_ops::IssueOut;
        let r = self.root.r;
        let node = |n: IssueNumber| r.item(n).map(|it| obj(Node { r, it })).unwrap_or(Out::Null);
        Some(match (f, self.root.out) {
            ("clientMutationId", _) => leaf(Value::Null),
            ("issue" | "pullRequest", IssueOut::Number(n)) => node(*n),
            ("commentEdge", IssueOut::Id(id)) => obj(Edge { r, n: self.root.p.subject?, id: *id }),
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

/// The payload of a committed `mergePullRequest`. gh selects only `clientMutationId`. The `Merge` command and its
/// `&Acked` are M2-WP3's; any committed output renders the same payload.
pub fn render_merge<T>(req: &Request<'_>, _acked: &Acked<T>) -> Value {
    req.run(&Merged)
}

struct Merged;

impl Obj for Merged {
    fn type_name(&self) -> &'static str {
        "Mutation"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "mergePullRequest").then(|| obj(MergedPayload))
    }
}

struct MergedPayload;

impl Obj for MergedPayload {
    fn type_name(&self) -> &'static str {
        "MergePullRequestPayload"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "clientMutationId").then(|| leaf(Value::Null))
    }
}
