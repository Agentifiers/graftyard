//! The GraphQL objects gh's `pr` and `issue` documents reach, over one repository's tiers. Each `Obj` resolves only
//! the fields those documents select (`gh/*.json`); any other field is `undefinedField`.
//!
//! Where GitHub's data has no row in the fold, the field answers what an install that never had the feature answers:
//! projects, milestones, issue types, sub-issues, reactions, and merge queues are later tiers or not served
//! (`design/SHARED.md`, "Surfaces, tiers, and oracles": `rest:projects` T2, `rest:reactions` T1), so they are empty or
//! null, never invented.

use graftyard_git::issues::RepoReleaseId;
use graftyard_git::issues::{
    CheckConclusion, CheckRunUpdate, CheckStatus, IssueKind, IssueNumber, MergeMethod, PrState, ReviewState,
    StateReason, StatusState, StatusUpdate,
};
use graftyard_git::refs::{ObjectId, RefName};
use graftyard_git::tiers::{CommentRow, LabelRow, ReleaseRow, ReviewRow, Timed};
use graftyard_identity::RepoPermission;
use graftyard_types::{AccountId, DbId, RepoId};
use serde_json::{Value, json};

use crate::exec::{Args, Obj, Out, leaf, obj};
use crate::view::{Item, Repo, RepoRef, StateFilter};
use crate::wire::{NodeId, NodeType, account_node, association, color, cursor, iso, parse_cursor, scoped};

fn branch(name: &RefName) -> String {
    let s = name.display();
    s.strip_prefix("refs/heads/").map(str::to_owned).unwrap_or(s)
}

fn first_or_last(a: &Args) -> usize {
    a.get("first").or_else(|| a.get("last")).and_then(Value::as_u64).map_or(100, |n| n.min(100) as usize)
}

// ---- connections ----

/// A connection page of objects: `nodes`, `totalCount`, `pageInfo`, and whatever `extra` adds.
struct Conn<'r> {
    ty: &'static str,
    nodes: Vec<Box<dyn Obj + 'r>>,
    total: usize,
    more: bool,
    end: Option<String>,
    extra: Vec<(&'static str, Value)>,
}

impl<'r> Conn<'r> {
    fn all(ty: &'static str, nodes: Vec<Box<dyn Obj + 'r>>) -> Conn<'r> {
        let total = nodes.len();
        let end = (total > 0).then(|| cursor(total as i64));
        Conn { ty, nodes, total, more: false, end, extra: Vec::new() }
    }

    /// The first or last `n` of every node, as GitHub's `first:` and `last:` page.
    fn window(ty: &'static str, mut nodes: Vec<Box<dyn Obj + 'r>>, a: &Args) -> Conn<'r> {
        let total = nodes.len();
        let n = first_or_last(a);
        let more = total > n;
        if a.contains_key("last") && !a.contains_key("first") {
            nodes.drain(..total.saturating_sub(n));
        } else {
            nodes.truncate(n);
        }
        let end = (!nodes.is_empty()).then(|| cursor(nodes.len() as i64));
        Conn { ty, nodes, total, more, end, extra: Vec::new() }
    }
}

struct PageInfo {
    more: bool,
    end: Option<String>,
}

impl Obj for PageInfo {
    fn type_name(&self) -> &'static str {
        "PageInfo"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "hasNextPage" => leaf(self.more),
            "endCursor" => leaf(self.end.clone()),
            _ => return None,
        })
    }
}

impl Obj for Conn<'_> {
    fn type_name(&self) -> &'static str {
        self.ty
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "nodes" => Out::List(self.nodes.iter().map(|n| Box::new(&**n) as Box<dyn Obj + 's>).collect()),
            "totalCount" => leaf(self.total),
            "pageInfo" => obj(PageInfo { more: self.more, end: self.end.clone() }),
            _ => return self.extra.iter().find(|(k, _)| *k == f).map(|(_, v)| leaf(v.clone())),
        })
    }
}

// ---- accounts ----

/// A user or organization, as `Actor`, `RepositoryOwner`, or `RequestedReviewer`.
struct Account<'r> {
    r: &'r Repo<'r>,
    id: AccountId,
}

impl Obj for Account<'_> {
    fn type_name(&self) -> &'static str {
        if self.r.dir.is_org(self.id) { "Organization" } else { "User" }
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let org = self.r.dir.is_org(self.id);
        Some(match f {
            "login" => leaf(self.r.dir.login(self.id).unwrap_or_else(|| "ghost".into())),
            "id" => leaf(account_node(self.r.dir, self.id)),
            "name" => leaf(self.r.dir.name(self.id)),
            "databaseId" if !org => leaf(graftyard_wire::database_id(self.id.0.0)),
            _ => return None,
        })
    }
}

fn account<'r>(r: &'r Repo<'r>, id: Option<AccountId>) -> Out<'r> {
    match id {
        Some(id) => obj(Account { r, id }),
        None => Out::Null,
    }
}

// ---- labels, reactions, refs ----

struct Label {
    id: DbId,
    repo: RepoId,
    row: LabelRow,
}

impl Obj for Label {
    fn type_name(&self) -> &'static str {
        "Label"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "id" => leaf(NodeId::scoped(NodeType::Label, self.repo, self.id.0).encode()),
            "name" => leaf(&*self.row.name.0),
            "description" => leaf(self.row.description.as_deref()),
            "color" => leaf(color(self.row.color)),
            _ => return None,
        })
    }
}

fn labels<'r>(r: &'r Repo<'r>, rows: Vec<(DbId, LabelRow)>) -> Vec<Box<dyn Obj + 'r>> {
    rows.into_iter().map(|(id, row)| Box::new(Label { id, repo: r.at.id, row }) as Box<dyn Obj>).collect()
}

struct Branch(String);

impl Obj for Branch {
    fn type_name(&self) -> &'static str {
        "Ref"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "name").then(|| leaf(self.0.clone()))
    }
}

// ---- repository ----

/// `Repository`: the addressed one, or another of its network for a fork head.
pub struct Repository<'r> {
    pub r: &'r Repo<'r>,
    pub at: RepoRef,
}

impl<'r> Repository<'r> {
    fn home(&self) -> bool {
        self.at.id == self.r.at.id
    }
}

impl Obj for Repository<'_> {
    fn type_name(&self) -> &'static str {
        "Repository"
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        let r = self.r;
        let s = r.settings().filter(|_| self.home());
        let at = &self.at;
        Some(match f {
            "id" => leaf(NodeId::top(NodeType::Repository, at.id.0.0).encode()),
            "databaseId" => leaf(graftyard_wire::database_id(at.id.0.0)),
            "name" => leaf(&*at.name),
            "nameWithOwner" => leaf(format!("{}/{}", at.owner, at.name)),
            "owner" => obj(Account { r, id: at.owner_id }),
            "sshUrl" => leaf(format!("git@{}:{}/{}.git", r.host.name(), at.owner, at.name)),
            "description" => leaf(s.as_ref().and_then(|s| s.description.as_deref().map(str::to_owned))),
            "hasIssuesEnabled" => leaf(s.as_ref().is_none_or(|s| s.has_issues)),
            "hasWikiEnabled" => leaf(s.as_ref().is_some_and(|s| s.has_wiki)),
            "defaultBranchRef" => match &s {
                Some(s) => obj(Branch(branch(&s.default_branch))),
                None => Out::Null,
            },
            "parent" => Out::Null,
            "viewerPermission" => leaf(r.dir.viewer().and_then(|v| r.dir.permission(v)).map(permission)),
            "mergeCommitAllowed" | "squashMergeAllowed" | "rebaseMergeAllowed" => {
                let m = match f {
                    "mergeCommitAllowed" => MergeMethod::Merge,
                    "squashMergeAllowed" => MergeMethod::Squash,
                    _ => MergeMethod::Rebase,
                };
                leaf(s.as_ref().is_none_or(|s| s.merge_methods.contains(&m)))
            }
            _ if !self.home() => return None,
            "issue" | "pullRequest" | "issueOrPullRequest" => {
                let n = a.get("number").and_then(Value::as_u64).unwrap_or(0) as u32;
                let want = match f {
                    "issue" => Some(IssueKind::Issue),
                    "pullRequest" => Some(IssueKind::PullRequest),
                    _ => None,
                };
                match r.item(IssueNumber(n)).filter(|i| want.is_none_or(|k| i.row.kind == k)) {
                    Some(it) => obj(Node { r, it }),
                    None => Out::Err(
                        "NOT_FOUND",
                        format!(
                            "Could not resolve to {} with the number of {n}.",
                            match f {
                                "pullRequest" => "a PullRequest",
                                "issue" => "an Issue",
                                _ => "an issue or pull request",
                            }
                        ),
                    ),
                }
            }
            "issues" => obj(list(r, IssueKind::Issue, a)),
            "pullRequests" => obj(list(r, IssueKind::PullRequest, a)),
            "labels" => obj(Conn::window("LabelConnection", labels(r, r.labels()), a)),
            // `release(tagName:)` finds drafts too, as gh's `RepositoryReleaseByTag` relies on for a draft's id.
            "release" => {
                let tag = a.get("tagName").and_then(Value::as_str).unwrap_or("");
                match r.release_by_tag(tag) {
                    Some((id, row)) => obj(Release { r, id, row }),
                    None => Out::Null,
                }
            }
            "latestRelease" => match r.latest_release().and_then(|id| Some((id, r.release(id)?))) {
                Some((id, row)) => obj(Release { r, id, row }),
                None => Out::Null,
            },
            "releases" => obj(releases(r, a)),
            _ => return None,
        })
    }
}

fn permission(p: RepoPermission) -> &'static str {
    match p {
        RepoPermission::Read => "READ",
        RepoPermission::Triage => "TRIAGE",
        RepoPermission::Write => "WRITE",
        RepoPermission::Maintain => "MAINTAIN",
        RepoPermission::Admin => "ADMIN",
    }
}

/// `Repository.issues` and `Repository.pullRequests`: newest first, as gh asks with `orderBy: CREATED_AT DESC`.
/// The order is by number, which is creation order on one chain.
fn list<'r>(r: &'r Repo<'r>, kind: IssueKind, a: &Args) -> Conn<'r> {
    let states: Vec<String> = a
        .get("states")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let has = |s: &str| states.is_empty() || states.iter().any(|x| x == s);
    let state = match (has("OPEN"), has("CLOSED") || has("MERGED")) {
        (true, false) => StateFilter::Open,
        (false, true) => StateFilter::Closed,
        _ => StateFilter::All,
    };
    let filter = a.get("filterBy").cloned().unwrap_or(Value::Null);
    let login = |k: &str| filter.get(k).and_then(Value::as_str).map(|l| r.dir.by_login(l));
    let (assignee, author, mention) = (login("assignee"), login("createdBy"), login("mentioned"));
    let head = a.get("headRefName").and_then(Value::as_str).map(str::to_owned);
    let base = a.get("baseRefName").and_then(Value::as_str).map(str::to_owned);
    let keep = |it: &Item| {
        let state_ok = match it.pr_state() {
            None => true,
            Some(PrState::Open { .. }) => has("OPEN"),
            Some(PrState::Closed { .. }) => has("CLOSED"),
            Some(PrState::Merged { .. }) => has("MERGED"),
        };
        let pull = it.pull.as_ref();
        state_ok
            && author.is_none_or(|w| w.is_some() && w == it.row.author)
            && assignee.is_none_or(|w| w.is_some_and(|w| r.assignees_of(it.n).contains(&w)))
            // `mentioned` needs the mention index, which nothing writes yet; no item matches it.
            && mention.is_none()
            && head.as_deref().is_none_or(|h| pull.is_some_and(|(p, _)| branch(&p.head.branch) == h))
            && base.as_deref().is_none_or(|b| pull.is_some_and(|(p, _)| branch(&p.base) == b))
    };
    let after = a.get("after").and_then(Value::as_str).and_then(parse_cursor).map(|n| IssueNumber(n as u32));
    let (all, _) = r.list(kind, state, None, usize::MAX, &keep);
    let total = all.len();
    let page: Vec<Item> = all.into_iter().filter(|i| after.is_none_or(|a| i.n < a)).collect();
    let n = first_or_last(a);
    let more = page.len() > n;
    let page: Vec<Item> = page.into_iter().take(n).collect();
    let end = page.last().map(|i| cursor(i64::from(i.n.0)));
    let ty = if kind == IssueKind::Issue { "IssueConnection" } else { "PullRequestConnection" };
    let nodes = page.into_iter().map(|it| Box::new(Node { r, it }) as Box<dyn Obj + 'r>).collect();
    Conn { ty, nodes, total, more, end, extra: Vec::new() }
}

// ---- issues and pull requests ----

/// An `Issue` or a `PullRequest`, whichever the item is.
pub struct Node<'r> {
    pub r: &'r Repo<'r>,
    pub it: Item,
}

impl Node<'_> {
    fn pull_number_id(&self) -> String {
        scoped(if self.it.is_pull() { NodeType::PullRequest } else { NodeType::Issue }, self.r, self.it.row.id.0)
    }
}

impl Obj for Node<'_> {
    fn type_name(&self) -> &'static str {
        if self.it.is_pull() { "PullRequest" } else { "Issue" }
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        let (r, it) = (self.r, &self.it);
        let n = it.n;
        let path = if it.is_pull() { "pull" } else { "issues" };
        let common = match f {
            "id" => Some(leaf(self.pull_number_id())),
            "number" => Some(leaf(n.0)),
            "title" => Some(leaf(&*it.row.title)),
            "body" => Some(leaf(&*it.row.body)),
            "url" => Some(leaf(format!("{}/{}/{}/{path}/{}", r.host.web(), r.at.owner, r.at.name, n.0))),
            "createdAt" => Some(leaf(iso(it.row.created))),
            "updatedAt" => Some(leaf(iso(it.row.updated))),
            "author" => Some(account(r, it.row.author)),
            "milestone" => Some(Out::Null),
            "assignees" => {
                let nodes = r.assignees_of(n).into_iter().map(|id| Box::new(Account { r, id }) as Box<dyn Obj>);
                Some(obj(Conn::window("UserConnection", nodes.collect(), a)))
            }
            "labels" => Some(obj(Conn::window("LabelConnection", labels(r, r.labels_of(n)), a))),
            "reactionGroups" => Some(Out::List(Vec::new())),
            "comments" => {
                let nodes = r
                    .comments_of(n)
                    .into_iter()
                    .map(|(id, row)| Box::new(Comment { r, id: id.0, row, on_pull: it.is_pull(), n }) as Box<dyn Obj>);
                Some(obj(Conn::window("IssueCommentConnection", nodes.collect(), a)))
            }
            "projectItems" => Some(obj(Conn::all("ProjectV2ItemConnection", Vec::new()))),
            _ => None,
        };
        if common.is_some() {
            return common;
        }
        match &it.pull {
            None => self.issue_field(f),
            Some(_) => self.pull_field(f, a),
        }
    }
}

impl<'r> Node<'r> {
    fn issue_field<'s>(&'s self, f: &str) -> Option<Out<'s>> {
        let row = &self.it.row;
        Some(match f {
            "state" => leaf(if row.open { "OPEN" } else { "CLOSED" }),
            "stateReason" => leaf(row.reason.map(|r| match r {
                StateReason::Completed => "COMPLETED",
                StateReason::NotPlanned => "NOT_PLANNED",
                StateReason::Reopened => "REOPENED",
            })),
            "issueType" | "parent" => Out::Null,
            "subIssues" | "blockedBy" | "blocking" => obj(Conn::all("IssueConnection", Vec::new())),
            "subIssuesSummary" => obj(SubIssues),
            _ => return None,
        })
    }

    fn pull_field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        let r = self.r;
        let n = self.it.n;
        let (pull, head) = self.it.pull.as_ref().expect("a pull request");
        let fork = pull.head.repo != r.at.id;
        let head_repo = if fork { r.dir.repo(pull.head.repo) } else { Some(r.at.clone()) };
        Some(match f {
            "state" => leaf(match head.state {
                PrState::Open { .. } => "OPEN",
                PrState::Closed { .. } => "CLOSED",
                PrState::Merged { .. } => "MERGED",
            }),
            "isDraft" => leaf(matches!(head.state, PrState::Open { draft: true })),
            "headRefName" => leaf(branch(&pull.head.branch)),
            "baseRefName" => leaf(branch(&pull.base)),
            "headRefOid" => leaf(head.head_sha.hex()),
            "isCrossRepository" => leaf(fork),
            "headRepository" => match head_repo {
                Some(at) => obj(Repository { r, at }),
                None => Out::Null,
            },
            "headRepositoryOwner" => match head_repo {
                Some(at) => obj(Account { r, id: at.owner_id }),
                None => Out::Null,
            },
            "maintainerCanModify" => leaf(false),
            "autoMergeRequest" => Out::Null,
            "isInMergeQueue" => leaf(false),
            "isMergeQueueEnabled" => leaf(r.protections(&pull.base).iter().any(|p| p.merge_queue.is_some())),
            "mergeable" => leaf(self.mergeable()),
            "mergeStateStatus" => leaf(self.merge_state()),
            "reviewDecision" => leaf(self.review_decision()),
            "additions" | "deletions" => {
                let c = r.dir.compare(pull.base_sha, head.head_sha);
                leaf(if f == "additions" { c.additions } else { c.deletions })
            }
            "commits" => obj(Commits {
                r,
                base: pull.base.clone(),
                head: head.head_sha,
                a: a.clone(),
                total: { r.dir.compare(pull.base_sha, head.head_sha).commits },
            }),
            "reviews" => {
                let nodes = r
                    .reviews_of(n)
                    .into_iter()
                    .filter(|(_, v)| v.state != ReviewState::Pending)
                    .map(|(id, row)| Box::new(Review { r, id, row }) as Box<dyn Obj>);
                obj(Conn::window("PullRequestReviewConnection", nodes.collect(), a))
            }
            "reviewRequests" => {
                let nodes = r.review_requests_of(n).into_iter().map(|g| Box::new(Request { r, g }) as Box<dyn Obj>);
                obj(Conn::window("ReviewRequestConnection", nodes.collect(), a))
            }
            _ => return None,
        })
    }

    /// The recorded test merge (`TestMerged`), while it is of the pull request's current head and its base's current
    /// tip (`graftyard_git::prsync::mergeable`); `None` until `PrSync` records the next one.
    fn test_merge(&self) -> Option<bool> {
        let (pull, head) = self.it.pull.as_ref().expect("a pull request");
        let m = graftyard_git::tiers::mergeability(self.r.core, self.it.n);
        graftyard_git::prsync::mergeable(m.as_ref(), head.head_sha, self.r.ref_value(&pull.base))
    }

    /// GraphQL's `mergeable`: `MERGEABLE` or `CONFLICTING` from the current test merge, `UNKNOWN` until one is recorded.
    fn mergeable(&self) -> &'static str {
        match self.test_merge() {
            Some(true) => "MERGEABLE",
            Some(false) => "CONFLICTING",
            None => "UNKNOWN",
        }
    }

    /// GraphQL's `mergeStateStatus`, from what `check_protection` reads in Core and the test merge: a draft, then
    /// a conflicting test merge (`DIRTY`), then failing or missing required checks and reviews and unresolved threads
    /// (`BLOCKED`), then `CLEAN` on a clean test merge, else `UNKNOWN` while the current one has not run.
    fn merge_state(&self) -> &'static str {
        let (pull, head) = self.it.pull.as_ref().expect("a pull request");
        if matches!(head.state, PrState::Open { draft: true }) {
            return "DRAFT";
        }
        let merge = self.test_merge();
        if merge == Some(false) {
            return "DIRTY";
        }
        let r = self.r;
        let ps = r.protections(&pull.base);
        let req = r.required(&head.head_sha);
        let checks_ok = ps.iter().filter_map(|p| p.checks.as_ref()).all(|c| {
            c.checks.iter().all(|(name, _)| {
                req.iter().any(|(n, row)| n == &**name && row.conclusion.is_some_and(CheckConclusion::passes))
            })
        });
        let approvals = r.verdicts(self.it.n).iter().filter(|v| v.state == ReviewState::Approved).count();
        let reviews_ok = ps.iter().filter_map(|p| p.reviews.as_ref()).all(|v| approvals >= usize::from(v.count));
        let threads_ok = !ps.iter().any(|p| p.conversation_resolution) || r.unresolved_threads(self.it.n) == 0;
        if !(checks_ok && reviews_ok && threads_ok) {
            return "BLOCKED";
        }
        if merge == Some(true) { "CLEAN" } else { "UNKNOWN" }
    }

    fn review_decision(&self) -> Option<&'static str> {
        let (pull, _) = self.it.pull.as_ref().expect("a pull request");
        let need = self.r.protections(&pull.base).iter().filter_map(|p| p.reviews.as_ref().map(|v| v.count)).max()?;
        let v = self.r.verdicts(self.it.n);
        Some(if v.iter().any(|v| v.state == ReviewState::ChangesRequested) {
            "CHANGES_REQUESTED"
        } else if v.iter().filter(|v| v.state == ReviewState::Approved).count() >= usize::from(need) {
            "APPROVED"
        } else {
            "REVIEW_REQUIRED"
        })
    }
}

struct SubIssues;

impl Obj for SubIssues {
    fn type_name(&self) -> &'static str {
        "SubIssuesSummary"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        matches!(f, "total" | "completed" | "percentCompleted").then(|| leaf(0))
    }
}

// ---- comments, reviews, review requests ----

pub fn comment<'r>(r: &'r Repo<'r>, id: DbId, row: CommentRow, on_pull: bool, n: IssueNumber) -> impl Obj + 'r {
    Comment { r, id, row, on_pull, n }
}

struct Comment<'r> {
    r: &'r Repo<'r>,
    id: DbId,
    row: CommentRow,
    on_pull: bool,
    n: IssueNumber,
}

impl Obj for Comment<'_> {
    fn type_name(&self) -> &'static str {
        "IssueComment"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let r = self.r;
        let me = r.dir.viewer();
        Some(match f {
            "id" => leaf(scoped(NodeType::IssueComment, r, self.id.0)),
            "author" => account(r, self.row.author),
            "authorAssociation" => leaf(association(r, self.row.author)),
            "body" => leaf(&*self.row.body),
            "createdAt" => leaf(iso(self.row.created)),
            "includesCreatedEdit" => leaf(self.row.updated != self.row.created),
            "isMinimized" => leaf(false),
            "minimizedReason" => Out::Null,
            "reactionGroups" => Out::List(Vec::new()),
            "url" => leaf(format!(
                "{}/{}/{}/{}/{}#issuecomment-{}",
                r.host.web(),
                r.at.owner,
                r.at.name,
                if self.on_pull { "pull" } else { "issues" },
                self.n.0,
                self.id.0
            )),
            "viewerDidAuthor" => leaf(me.is_some() && me == self.row.author),
            _ => return None,
        })
    }
}

struct Review<'r> {
    r: &'r Repo<'r>,
    id: DbId,
    row: ReviewRow,
}

impl Obj for Review<'_> {
    fn type_name(&self) -> &'static str {
        "PullRequestReview"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let r = self.r;
        Some(match f {
            "id" => leaf(scoped(NodeType::PullRequestReview, r, self.id.0)),
            "author" => account(r, self.row.reviewer),
            "authorAssociation" => leaf(association(r, self.row.reviewer)),
            "submittedAt" => leaf(iso(self.row.at)),
            "body" => leaf(&*self.row.body),
            "state" => leaf(match (self.row.dismissed.is_some(), self.row.state) {
                (true, _) => "DISMISSED",
                (_, ReviewState::Pending) => "PENDING",
                (_, ReviewState::Approved) => "APPROVED",
                (_, ReviewState::ChangesRequested) => "CHANGES_REQUESTED",
                (_, ReviewState::Commented) => "COMMENTED",
            }),
            "commit" => obj(CommitObj { r, oid: self.row.commit, base: None }),
            "reactionGroups" => Out::List(Vec::new()),
            _ => return None,
        })
    }
}

/// A `ReviewRequest`, from its `review_request` key: `user:<id>` or `team:<id>`.
struct Request<'r> {
    r: &'r Repo<'r>,
    g: String,
}

struct Team {
    org: String,
    name: String,
    slug: String,
}

impl Obj for Team {
    fn type_name(&self) -> &'static str {
        "Team"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "name" => leaf(self.name.clone()),
            "slug" => leaf(self.slug.clone()),
            "organization" => obj(OrgLogin(self.org.clone())),
            _ => return None,
        })
    }
}

struct OrgLogin(String);

impl Obj for OrgLogin {
    fn type_name(&self) -> &'static str {
        "Organization"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "login").then(|| leaf(self.0.clone()))
    }
}

impl Obj for Request<'_> {
    fn type_name(&self) -> &'static str {
        "ReviewRequest"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        if f != "requestedReviewer" {
            return None;
        }
        let (kind, id) = self.g.split_once(':')?;
        let id: i64 = id.parse().ok()?;
        Some(match kind {
            "user" => obj(Account { r: self.r, id: AccountId(DbId(id)) }),
            _ => match self.r.dir.team(DbId(id)) {
                Some((org, name, slug)) => obj(Team { org, name, slug }),
                None => Out::Null,
            },
        })
    }
}

// ---- commits and checks ----

/// `PullRequest.commits`: gh asks only for the head commit (`last: 1`) and the count.
struct Commits<'r> {
    r: &'r Repo<'r>,
    base: RefName,
    head: ObjectId,
    a: Args,
    total: u32,
}

impl Obj for Commits<'_> {
    fn type_name(&self) -> &'static str {
        "PullRequestCommitConnection"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let _ = &self.a;
        Some(match f {
            "totalCount" => leaf(self.total),
            "nodes" => Out::List(vec![Box::new(PrCommit(CommitObj {
                r: self.r,
                oid: self.head,
                base: Some(self.base.clone()),
            }))]),
            _ => return None,
        })
    }
}

struct PrCommit<'r>(CommitObj<'r>);

impl Obj for PrCommit<'_> {
    fn type_name(&self) -> &'static str {
        "PullRequestCommit"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "commit").then(|| obj(&self.0))
    }
}

/// A `Commit`, with the base branch whose rules decide `isRequired` when it is a pull request's head.
struct CommitObj<'r> {
    r: &'r Repo<'r>,
    oid: ObjectId,
    base: Option<RefName>,
}

impl Obj for CommitObj<'_> {
    fn type_name(&self) -> &'static str {
        "Commit"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        Some(match f {
            "oid" => leaf(self.oid.hex()),
            "statusCheckRollup" => {
                let runs = self.r.check_runs(&self.oid);
                let statuses = latest_statuses(self.r, &self.oid);
                if runs.is_empty() && statuses.is_empty() {
                    Out::Null
                } else {
                    obj(Rollup { r: self.r, base: self.base.clone(), runs, statuses })
                }
            }
            _ => return None,
        })
    }
}

/// The latest status per context, as the rollup and the combined status show them.
pub fn latest_statuses(r: &Repo<'_>, sha: &ObjectId) -> Vec<Timed<StatusUpdate>> {
    let mut seen = std::collections::BTreeSet::new();
    r.statuses(sha, 0, usize::MAX).into_iter().filter(|s| seen.insert(s.rec.context.clone())).collect()
}

/// The latest run per (app, name): a re-run replaces the run it reruns in the rollup.
fn latest_runs(runs: Vec<Timed<CheckRunUpdate>>) -> Vec<Timed<CheckRunUpdate>> {
    let mut by: std::collections::BTreeMap<(i64, Box<str>), Timed<CheckRunUpdate>> = Default::default();
    for t in runs {
        let k = (t.rec.app.0.0, t.rec.name.clone());
        if by.get(&k).is_none_or(|o| o.rec.run < t.rec.run) {
            by.insert(k, t);
        }
    }
    by.into_values().collect()
}

struct Rollup<'r> {
    r: &'r Repo<'r>,
    base: Option<RefName>,
    runs: Vec<Timed<CheckRunUpdate>>,
    statuses: Vec<Timed<StatusUpdate>>,
}

impl Obj for Rollup<'_> {
    fn type_name(&self) -> &'static str {
        "StatusCheckRollup"
    }
    fn field<'s>(&'s self, f: &str, a: &Args) -> Option<Out<'s>> {
        if f != "contexts" {
            return None;
        }
        let mut nodes: Vec<Box<dyn Obj + 's>> = Vec::new();
        for t in latest_runs(self.runs.clone()) {
            nodes.push(Box::new(Run { r: self.r, base: self.base.clone(), t }));
        }
        for t in &self.statuses {
            nodes.push(Box::new(Status { r: self.r, base: self.base.clone(), t: t.clone() }));
        }
        // `after` continues from a position this connection's own `endCursor` gave.
        let skip = a.get("after").and_then(Value::as_str).and_then(parse_cursor).unwrap_or(0).max(0) as usize;
        let total = nodes.len();
        let nodes: Vec<Box<dyn Obj + 's>> = nodes.into_iter().skip(skip).collect();
        let n = first_or_last(a);
        let more = nodes.len() > n;
        let nodes: Vec<Box<dyn Obj + 's>> = nodes.into_iter().take(n).collect();
        let end = (!nodes.is_empty()).then(|| cursor((skip + nodes.len()) as i64));
        Some(obj(Conn { ty: "StatusCheckRollupContextConnection", nodes, total, more, end, extra: Vec::new() }))
    }
}

struct Run<'r> {
    r: &'r Repo<'r>,
    base: Option<RefName>,
    t: Timed<CheckRunUpdate>,
}

fn check_status(s: CheckStatus) -> &'static str {
    match s {
        CheckStatus::Queued => "QUEUED",
        CheckStatus::InProgress => "IN_PROGRESS",
        CheckStatus::Completed => "COMPLETED",
        CheckStatus::Waiting => "WAITING",
        CheckStatus::Requested => "REQUESTED",
        CheckStatus::Pending => "PENDING",
    }
}

pub fn conclusion(c: CheckConclusion) -> &'static str {
    match c {
        CheckConclusion::ActionRequired => "ACTION_REQUIRED",
        CheckConclusion::Cancelled => "CANCELLED",
        CheckConclusion::Failure => "FAILURE",
        CheckConclusion::Neutral => "NEUTRAL",
        CheckConclusion::Success => "SUCCESS",
        CheckConclusion::Skipped => "SKIPPED",
        CheckConclusion::Stale => "STALE",
        CheckConclusion::TimedOut => "TIMED_OUT",
        CheckConclusion::StartupFailure => "STARTUP_FAILURE",
    }
}

impl Obj for Run<'_> {
    fn type_name(&self) -> &'static str {
        "CheckRun"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let c = &self.t.rec;
        Some(match f {
            "name" => leaf(&*c.name),
            "status" => leaf(check_status(c.status)),
            "conclusion" => leaf(c.conclusion.map(conclusion)),
            "startedAt" => leaf(c.started.map(iso)),
            "completedAt" => leaf(c.completed.map(iso)),
            "detailsUrl" => leaf(c.details_url.as_ref().map(|u| u.0.to_string())),
            // Actions workflow runs arrive with Milestone 4; until then a run belongs to no workflow run.
            "checkSuite" => obj(Suite),
            "isRequired" => leaf(self.base.as_ref().is_some_and(|b| self.r.is_required(b, &c.name, Some(c.app)))),
            _ => return None,
        })
    }
}

struct Suite;

impl Obj for Suite {
    fn type_name(&self) -> &'static str {
        "CheckSuite"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        (f == "workflowRun").then_some(Out::Null)
    }
}

struct Status<'r> {
    r: &'r Repo<'r>,
    base: Option<RefName>,
    t: Timed<StatusUpdate>,
}

impl Obj for Status<'_> {
    fn type_name(&self) -> &'static str {
        "StatusContext"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let s = &self.t.rec;
        Some(match f {
            "context" => leaf(&*s.context),
            "state" => leaf(match s.state {
                StatusState::Error => "ERROR",
                StatusState::Failure => "FAILURE",
                StatusState::Pending => "PENDING",
                StatusState::Success => "SUCCESS",
            }),
            "targetUrl" => leaf(s.target_url.as_ref().map(|u| u.0.to_string())),
            "description" => leaf(s.description.as_deref()),
            "createdAt" => leaf(iso(self.t.at)),
            "isRequired" => leaf(self.base.as_ref().is_some_and(|b| self.r.is_required(b, &s.context, None))),
            _ => return None,
        })
    }
}

// ---- search ----

/// `Query.search` with `type: ISSUE_ADVANCED`, for the qualifiers gh's list commands write: `repo:`, `type:`,
/// `state:`, `label:`, `author:`, `assignee:`, `draft:`. A query with any other qualifier or free text is refused
/// rather than answered wrong; repository search beyond these is `rest:search` at T1.
pub fn search<'r>(r: &'r Repo<'r>, a: &Args) -> Out<'r> {
    let q = a.get("query").and_then(Value::as_str).unwrap_or("");
    let mut kind = None;
    let mut state = StateFilter::All;
    let (mut labels, mut author, mut assignee, mut draft) = (Vec::new(), None, None, None);
    for term in q.split_whitespace() {
        let (k, v) = term.split_once(':').unwrap_or(("", term));
        let v = v.trim_matches('"');
        match k {
            "repo" if v == format!("{}/{}", r.at.owner, r.at.name) => {}
            "type" | "is" if v == "issue" => kind = Some(IssueKind::Issue),
            "type" | "is" if v == "pr" => kind = Some(IssueKind::PullRequest),
            "state" if v == "open" => state = StateFilter::Open,
            "state" if v == "closed" => state = StateFilter::Closed,
            "label" => labels.push(v.to_lowercase()),
            "author" => author = Some(r.dir.by_login(v)),
            "assignee" => assignee = Some(r.dir.by_login(v)),
            "draft" => draft = Some(v == "true"),
            _ => return Out::Err("INVALID_QUERY", format!("search qualifier '{term}' is not served")),
        }
    }
    let Some(kind) = kind else { return Out::Err("INVALID_QUERY", "search needs type:issue or type:pr".into()) };
    let keep = |it: &Item| {
        let names: Vec<String> = r.labels_of(it.n).into_iter().map(|(_, l)| l.name.0.to_lowercase()).collect();
        labels.iter().all(|l| names.contains(l))
            && author.is_none_or(|w| w.is_some() && w == it.row.author)
            && assignee.is_none_or(|w| w.is_some_and(|w| r.assignees_of(it.n).contains(&w)))
            && draft.is_none_or(|d| matches!(it.pr_state(), Some(PrState::Open { draft }) if draft == d))
    };
    let (all, _) = r.list(kind, state, None, usize::MAX, &keep);
    let total = all.len();
    let nodes: Vec<Box<dyn Obj + 'r>> = all.into_iter().map(|it| Box::new(Node { r, it }) as Box<dyn Obj>).collect();
    let mut c = Conn::window("SearchResultItemConnection", nodes, a);
    c.extra.push(("issueCount", json!(total)));
    obj(c)
}

// ---- releases ----

/// `Release`, for gh's `RepositoryReleaseByTag` and `RepositoryReleaseList`.
struct Release<'r> {
    r: &'r Repo<'r>,
    id: RepoReleaseId,
    row: ReleaseRow,
}

impl Obj for Release<'_> {
    fn type_name(&self) -> &'static str {
        "Release"
    }
    fn field<'s>(&'s self, f: &str, _: &Args) -> Option<Out<'s>> {
        let (r, rec) = (self.r, &self.row.rec);
        let tag = rec.tag.display();
        let tag = tag.strip_prefix("refs/tags/").map(str::to_owned).unwrap_or(tag);
        Some(match f {
            "id" => leaf(scoped(NodeType::Release, r, self.id.0.0)),
            "databaseId" => leaf(graftyard_wire::database_id(self.id.0.0)),
            "name" => leaf((!rec.name.is_empty()).then(|| rec.name.to_string())),
            "description" => leaf((!rec.body.is_empty()).then(|| rec.body.to_string())),
            "tagName" => leaf(tag),
            "isDraft" => leaf(rec.draft),
            "isPrerelease" => leaf(rec.prerelease),
            "isLatest" => leaf(r.latest_release() == Some(self.id)),
            "immutable" => leaf(false),
            "createdAt" => leaf(iso(self.row.created)),
            "publishedAt" => leaf(self.row.published.map(iso)),
            "author" => account(r, rec.author),
            _ => return None,
        })
    }
}

/// `Repository.releases(first:, after:, orderBy: {field: CREATED_AT, direction:})`, keyed by id, which grows with
/// creation. `NAME` order is not one gh sends.
fn releases<'r>(r: &'r Repo<'r>, a: &Args) -> Conn<'r> {
    let mut all = r.releases();
    if a.get("orderBy").and_then(|o| o.get("direction")).and_then(Value::as_str) == Some("ASC") {
        all.reverse();
    }
    let total = all.len();
    let after = a.get("after").and_then(Value::as_str).and_then(parse_cursor);
    let start = after.map_or(0, |c| c as usize);
    let n = first_or_last(a);
    let page: Vec<(RepoReleaseId, ReleaseRow)> = all.into_iter().skip(start).take(n + 1).collect();
    let more = page.len() > n;
    let nodes: Vec<Box<dyn Obj + 'r>> =
        page.into_iter().take(n).map(|(id, row)| Box::new(Release { r, id, row }) as Box<dyn Obj + 'r>).collect();
    let end = (!nodes.is_empty()).then(|| cursor((start + nodes.len()) as i64));
    Conn { ty: "ReleaseConnection", nodes, total, more, end, extra: Vec::new() }
}
