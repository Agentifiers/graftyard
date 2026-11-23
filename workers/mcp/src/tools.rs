//! `graft-mcp`'s tools (`design/AGENT-LAYER.md`, Shape, "graft-mcp"; journey A7, step 7; `build/BUILD-PLAN.md`,
//! AM2-WP2, WP2a): the static toolsets, `call`, and `ToolError`.
//!
//! - **One to three REST calls each, through the edge, with the scoped `upstream` token,** so rulesets, `decide`, and
//!   `Authz` rule and `graft-mcp` decides nothing about access. Reads are the edge's REST routes, writes its GraphQL
//!   mutations (the edge serves `createIssue`, `createPullRequest`, `addComment`, `addPullRequestReview`, and
//!   `mergePullRequest` there, and no REST route for them), and leases and the feed are `/api/graftyard/v1`.
//! - **The trust view on every text field.** Every read asks for `AGENT_MEDIA_TYPE`, so each issue, pull request,
//!   comment, review, commit, and check run carries the edge's `graftyard_trust`. A tool returns that view's `visible`
//!   in the field's place and keeps the raw text out of what the model reads (`hidden` and `author` stay beside it).
//!   A text field the edge did not view (a lease's `intent` is already a `TrustView`) is never passed through raw.
//! - **Repositories are the grant's.** A tool naming any other repository answers 404 before any call, as the host
//!   would for a token scoped to the grant's repositories.
//! - **Write scope.** Write tools exist only on a `Write` token, and approve and merge only on `WriteApproveMerge`; the
//!   host refuses them anyway from the scoped token's row.
//! - **`ToolError`** carries the edge's status and message, `retry_after_s` from `Retry-After`, and the
//!   `X-RateLimit-*` fields, so a client can back off without parsing text (`research/agents.md` 3.8).
//! - **Writes run as `Graftyard MCP`** with `requester = sub`: the scoped token is that App's user-to-server token, so
//!   the host fills `actor_is_agent` and the agent admission class. A lease names its task in
//!   `X-Graftyard-Agent-Session` when the caller gives one.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{McpToken, Toolset, WriteScope};

/// The edge's opt-in trust view (`graftyard_api::trust::AGENT_MEDIA_TYPE`).
pub const AGENT_MEDIA_TYPE: &str = "application/vnd.graftyard.agent+json";
/// The longest lease wait a tool asks the feed for: the feed's own cap (`graftyard_git::feed::FEED_WAIT_MAX`), so a
/// tool call ends inside a client's timeout [guess: SDK clients' default timeouts are 60 s].
pub const FEED_WAIT_MAX_S: u64 = 25;

/// What the edge answered: status, headers (lower-case names), and body.
#[derive(Clone, Debug, Default)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Answer {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// The edge's rate fields on an error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RateInfo {
    pub limit: Option<u64>,
    pub remaining: Option<u64>,
    pub reset: Option<u64>,
    pub used: Option<u64>,
}

/// A failed tool call (Shape, "graft-mcp"): the HTTP status behind it, the host's message, and how long to wait.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ToolError {
    pub status: u16,
    pub message: Box<str>,
    pub retry_after_s: Option<u32>,
    pub rate: Option<RateInfo>,
}

impl ToolError {
    fn new(status: u16, message: impl Into<Box<str>>) -> ToolError {
        ToolError { status, message: message.into(), retry_after_s: None, rate: None }
    }

    /// From an edge answer outside 2xx, or a GraphQL `errors` list.
    fn of(a: &Answer, message: String) -> ToolError {
        let n = |k: &str| a.header(k).and_then(|v| v.trim().parse::<u64>().ok());
        let rate = n("x-ratelimit-limit").map(|limit| RateInfo {
            limit: Some(limit),
            remaining: n("x-ratelimit-remaining"),
            reset: n("x-ratelimit-reset"),
            used: n("x-ratelimit-used"),
        });
        let retry_after_s = n("retry-after").and_then(|s| u32::try_from(s).ok());
        ToolError { status: a.status, message: message.into(), retry_after_s, rate }
    }
}

/// One tool: its name, toolset, the write scope it needs, its description, and its input schema's properties (each a
/// JSON schema) with the required ones.
pub struct Tool {
    pub name: &'static str,
    pub set: Toolset,
    pub needs: WriteScope,
    pub description: &'static str,
    pub props: &'static [(&'static str, &'static str, &'static str)],
    pub required: &'static [&'static str],
}

const REPO: [(&str, &str, &str); 2] =
    [("owner", "string", "The repository's owner"), ("repo", "string", "The repository's name")];

macro_rules! props {
    ($($p:expr),* $(,)?) => { &[REPO[0], REPO[1], $($p),*] };
}

/// Every tool `graft-mcp` serves. A token sees those of its toolsets at or below its write scope.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "get_me",
        set: Toolset::Context,
        needs: WriteScope::Read,
        description: "The signed-in user, and the repositories, toolsets, and write scope this connection was granted",
        props: &[],
        required: &[],
    },
    Tool {
        name: "list_branches",
        set: Toolset::Repos,
        needs: WriteScope::Read,
        description: "A repository's branches",
        props: props!(("per_page", "integer", "At most 100"), ("page", "integer", "From 1")),
        required: &["owner", "repo"],
    },
    Tool {
        name: "get_commit",
        set: Toolset::Repos,
        needs: WriteScope::Read,
        description: "One commit, its message as the trust view shows it",
        props: props!(("ref", "string", "A sha, branch, or tag")),
        required: &["owner", "repo", "ref"],
    },
    Tool {
        name: "list_issues",
        set: Toolset::Issues,
        needs: WriteScope::Read,
        description: "A repository's issues and pull requests, newest first, bodies as the trust view shows them",
        props: props!(
            ("state", "string", "open, closed, or all"),
            ("labels", "string", "Comma-separated label names"),
            ("per_page", "integer", "At most 100"),
            ("page", "integer", "From 1"),
        ),
        required: &["owner", "repo"],
    },
    Tool {
        name: "list_issue_comments",
        set: Toolset::Issues,
        needs: WriteScope::Read,
        description: "Comments on an issue or pull request, as the trust view shows them",
        props: props!(
            ("number", "integer", "The issue or pull request number"),
            ("per_page", "integer", "At most 100"),
            ("page", "integer", "From 1")
        ),
        required: &["owner", "repo", "number"],
    },
    Tool {
        name: "create_issue",
        set: Toolset::Issues,
        needs: WriteScope::Write,
        description: "Open an issue",
        props: props!(("title", "string", "The title"), ("body", "string", "The body, markdown")),
        required: &["owner", "repo", "title"],
    },
    Tool {
        name: "add_issue_comment",
        set: Toolset::Issues,
        needs: WriteScope::Write,
        description: "Comment on an issue or pull request",
        props: props!(
            ("number", "integer", "The issue or pull request number"),
            ("body", "string", "The comment, markdown")
        ),
        required: &["owner", "repo", "number", "body"],
    },
    Tool {
        name: "get_pull_request",
        set: Toolset::PullRequests,
        needs: WriteScope::Read,
        description: "One pull request, its body as the trust view shows it",
        props: props!(("number", "integer", "The pull request number")),
        required: &["owner", "repo", "number"],
    },
    Tool {
        name: "list_pull_request_reviews",
        set: Toolset::PullRequests,
        needs: WriteScope::Read,
        description: "Reviews on a pull request, as the trust view shows them",
        props: props!(("number", "integer", "The pull request number")),
        required: &["owner", "repo", "number"],
    },
    Tool {
        name: "list_pull_request_review_comments",
        set: Toolset::PullRequests,
        needs: WriteScope::Read,
        description: "Review comments on a pull request's diff, as the trust view shows them",
        props: props!(("number", "integer", "The pull request number")),
        required: &["owner", "repo", "number"],
    },
    Tool {
        name: "create_pull_request",
        set: Toolset::PullRequests,
        needs: WriteScope::Write,
        description: "Open a pull request from a branch of this repository; a draft unless draft is false",
        props: props!(
            ("head", "string", "The branch with the changes"),
            ("base", "string", "The branch to merge into"),
            ("title", "string", "The title"),
            ("body", "string", "The body, markdown"),
            ("draft", "boolean", "Default true"),
        ),
        required: &["owner", "repo", "head", "base", "title"],
    },
    Tool {
        name: "approve_pull_request",
        set: Toolset::PullRequests,
        needs: WriteScope::WriteApproveMerge,
        description: "Approve a pull request at its head",
        props: props!(("number", "integer", "The pull request number"), ("body", "string", "A review comment")),
        required: &["owner", "repo", "number"],
    },
    Tool {
        name: "merge_pull_request",
        set: Toolset::PullRequests,
        needs: WriteScope::WriteApproveMerge,
        description: "Merge a pull request, only if its head is still sha",
        props: props!(
            ("number", "integer", "The pull request number"),
            ("sha", "string", "The head the pull request must be at"),
            ("method", "string", "merge, squash, or rebase"),
        ),
        required: &["owner", "repo", "number", "sha"],
    },
    Tool {
        name: "list_check_runs",
        set: Toolset::Checks,
        needs: WriteScope::Read,
        description: "Check runs for a ref, their output as the trust view shows it",
        props: props!(("ref", "string", "A sha, branch, or tag")),
        required: &["owner", "repo", "ref"],
    },
    Tool {
        name: "get_check_run",
        set: Toolset::Checks,
        needs: WriteScope::Read,
        description: "One check run, its output as the trust view shows it",
        props: props!(("id", "integer", "The check run id")),
        required: &["owner", "repo", "id"],
    },
    Tool {
        name: "coordination.lease",
        set: Toolset::Coordination,
        needs: WriteScope::Write,
        description: "Lease paths (and optionally refs) so other agents see the work; returns the lease and every live lease it overlaps",
        props: props!(
            ("paths", "array", "Path globs"),
            ("refs", "array", "Ref patterns"),
            ("mode", "string", "advisory or exclusive"),
            ("ttl_seconds", "integer", "How long it lasts"),
            ("intent", "string", "What the work is, shown to others"),
            ("session", "string", "The task's session id"),
        ),
        required: &["owner", "repo", "paths"],
    },
    Tool {
        name: "coordination.renew",
        set: Toolset::Coordination,
        needs: WriteScope::Write,
        description: "Renew a lease",
        props: props!(("id", "string", "The lease id"), ("ttl_seconds", "integer", "How long it lasts from now")),
        required: &["owner", "repo", "id"],
    },
    Tool {
        name: "coordination.release",
        set: Toolset::Coordination,
        needs: WriteScope::Write,
        description: "Release a lease",
        props: props!(("id", "string", "The lease id")),
        required: &["owner", "repo", "id"],
    },
    Tool {
        name: "coordination.leases",
        set: Toolset::Coordination,
        needs: WriteScope::Read,
        description: "The live leases, those overlapping paths when given",
        props: props!(("paths", "array", "Path globs")),
        required: &["owner", "repo"],
    },
    Tool {
        name: "coordination.activity",
        set: Toolset::Coordination,
        needs: WriteScope::Read,
        description: "The repository's activity feed after cursor, waiting up to wait seconds for a new event",
        props: props!(
            ("cursor", "string", "The cursor the last call returned"),
            ("wait", "integer", "Seconds to wait, at most 25"),
            ("paths", "array", "Only events touching these path globs"),
        ),
        required: &["owner", "repo"],
    },
    Tool {
        name: "coordination.overlaps",
        set: Toolset::Coordination,
        needs: WriteScope::Read,
        description: "Other agents' refs and live leases whose paths overlap a ref's, a pull request's, or the given \
            path globs. Each overlap's verdict is null until conflict prediction ships: null means not yet judged, \
            never no conflict",
        props: props!(
            ("ref", "string", "A branch whose pushed and planned paths are the subject"),
            ("pr", "integer", "A pull request whose head branch is the subject"),
            ("paths", "array", "Path globs to check"),
        ),
        required: &["owner", "repo"],
    },
    Tool {
        name: "provenance.why",
        set: Toolset::Provenance,
        needs: WriteScope::Read,
        description: "Why a line exists: the commit blame names for it, its pull request, the agent sessions behind it, \
            and its trailers, of which only sessions are linked",
        props: props!(
            ("ref", "string", "The branch, tag, or commit to blame at"),
            ("path", "string", "The file"),
            ("line", "integer", "The line, from 1"),
        ),
        required: &["owner", "repo", "ref", "path", "line"],
    },
];

/// Whether `tok` may see `t`.
pub fn visible_to(t: &Tool, tok: &McpToken) -> bool {
    tok.tools.has(t.set) && t.needs <= tok.write
}

/// `tools/list`'s `tools`: the token's tools, each with its JSON schema.
pub fn list(tok: &McpToken) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|t| visible_to(t, tok))
        .map(|t| {
            let props: Map<String, Value> = t
                .props
                .iter()
                .map(|(k, ty, d)| {
                    let s = match *ty {
                        "array" => json!({ "type": "array", "items": { "type": "string" }, "description": d }),
                        ty => json!({ "type": ty, "description": d }),
                    };
                    ((*k).to_owned(), s)
                })
                .collect();
            json!({ "name": t.name, "description": t.description,
                "inputSchema": { "type": "object", "properties": props, "required": t.required } })
        })
        .collect()
}

/// One request to the edge, and the edge itself: the Worker's `EDGE` binding or a test's members.
#[allow(async_fn_in_trait)]
pub trait Edge {
    async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<Answer, String>;
}

/// Runs `name` with `args` for `tok`'s user, through `edge`.
pub async fn call<E: Edge>(name: &str, args: &Value, tok: &McpToken, edge: &E) -> Result<Value, ToolError> {
    let Some(t) = TOOLS.iter().find(|t| t.name == name) else {
        return Err(ToolError::new(404, format!("no tool {name}")));
    };
    if !visible_to(t, tok) {
        // A tool outside the grant is absent from `tools/list`; calling it anyway is the host's 403.
        return Err(ToolError::new(403, format!("this connection was not granted {name}")));
    }
    for r in t.required {
        if args.get(*r).is_none_or(Value::is_null) {
            return Err(ToolError::new(422, format!("{r} is required")));
        }
    }
    let c = Caller { tok, edge };
    let s = |k: &str| args.get(k).and_then(Value::as_str);
    let n = |k: &str| args.get(k).and_then(Value::as_u64);
    let page = |q: &mut Vec<(&str, String)>| {
        for k in ["per_page", "page"] {
            if let Some(v) = n(k) {
                q.push((if k == "page" { "page" } else { "per_page" }, v.to_string()));
            }
        }
    };
    match name {
        "get_me" => {
            let me = c.get("/api/v3/user", &[]).await?;
            Ok(json!({ "login": me["login"], "id": me["id"], "repositories": tok.repos, "toolsets": tok.tools.wire(),
                "write": tok.write.wire() }))
        }
        "list_branches" => {
            let mut q = Vec::new();
            page(&mut q);
            c.get(&format!("{}/branches", c.repo(args)?), &q).await
        }
        "get_commit" => {
            let v = c.get(&format!("{}/commits/{}", c.repo(args)?, seg(s("ref").unwrap_or_default())), &[]).await?;
            Ok(trusted(v))
        }
        "list_issues" => {
            let mut q = Vec::new();
            for k in ["state", "labels"] {
                if let Some(v) = s(k) {
                    q.push((k, v.to_owned()));
                }
            }
            page(&mut q);
            Ok(trusted(c.get(&format!("{}/issues", c.repo(args)?), &q).await?))
        }
        "list_issue_comments" => {
            let mut q = Vec::new();
            page(&mut q);
            let path = format!("{}/issues/{}/comments", c.repo(args)?, num(args, "number")?);
            Ok(trusted(c.get(&path, &q).await?))
        }
        "get_pull_request" => {
            Ok(trusted(c.get(&format!("{}/pulls/{}", c.repo(args)?, num(args, "number")?), &[]).await?))
        }
        "list_pull_request_reviews" => {
            Ok(trusted(c.get(&format!("{}/pulls/{}/reviews", c.repo(args)?, num(args, "number")?), &[]).await?))
        }
        "list_pull_request_review_comments" => {
            Ok(trusted(c.get(&format!("{}/pulls/{}/comments", c.repo(args)?, num(args, "number")?), &[]).await?))
        }
        "list_check_runs" => {
            let path = format!("{}/commits/{}/check-runs", c.repo(args)?, seg(s("ref").unwrap_or_default()));
            Ok(trusted(c.get(&path, &[]).await?))
        }
        "get_check_run" => {
            Ok(trusted(c.get(&format!("{}/check-runs/{}", c.repo(args)?, num(args, "id")?), &[]).await?))
        }
        "create_issue" => {
            let repo_id = c.node(args).await?;
            let input = json!({ "repositoryId": repo_id, "title": s("title"), "body": s("body").unwrap_or("") });
            let v = c.mutate("createIssue", "issue { number url title }", input).await?;
            Ok(v["issue"].clone())
        }
        "add_issue_comment" => {
            let subject = c.item_node(args).await?;
            let input = json!({ "subjectId": subject, "body": s("body") });
            let v = c.mutate("addComment", "commentEdge { node { id url } }", input).await?;
            Ok(v["commentEdge"]["node"].clone())
        }
        "create_pull_request" => {
            let repo_id = c.node(args).await?;
            let input = json!({ "repositoryId": repo_id, "headRefName": s("head"), "baseRefName": s("base"),
                "title": s("title"), "body": s("body").unwrap_or(""),
                "draft": args.get("draft").and_then(Value::as_bool).unwrap_or(true) });
            let v = c.mutate("createPullRequest", "pullRequest { number url isDraft }", input).await?;
            Ok(v["pullRequest"].clone())
        }
        "approve_pull_request" => {
            let pr = c.item_node(args).await?;
            let input = json!({ "pullRequestId": pr, "event": "APPROVE", "body": s("body").unwrap_or("") });
            let v = c.mutate("addPullRequestReview", "pullRequestReview { id state }", input).await?;
            Ok(v["pullRequestReview"].clone())
        }
        "merge_pull_request" => {
            let pr = c.item_node(args).await?;
            let method = s("method").unwrap_or("merge").to_ascii_uppercase();
            let input = json!({ "pullRequestId": pr, "expectedHeadOid": s("sha"), "mergeMethod": method });
            let v = c.mutate("mergePullRequest", "pullRequest { number merged mergeCommit { oid } }", input).await?;
            Ok(v["pullRequest"].clone())
        }
        "coordination.leases" => {
            let mut q = Vec::new();
            for p in strs(args, "paths") {
                q.push(("paths[]", p));
            }
            c.agents("GET", &format!("{}/leases", c.agents_repo(args)?), &q, None, None).await
        }
        "coordination.lease" => {
            let mut body = Map::new();
            for k in ["paths", "refs", "mode", "ttl_seconds", "intent"] {
                if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
                    body.insert(k.into(), v.clone());
                }
            }
            let path = format!("{}/leases", c.agents_repo(args)?);
            c.agents("POST", &path, &[], Some(Value::Object(body)), s("session")).await
        }
        "coordination.renew" => {
            let body = json!({ "ttl_seconds": args.get("ttl_seconds") });
            let path = format!("{}/leases/{}/renew", c.agents_repo(args)?, seg(s("id").unwrap_or_default()));
            c.agents("POST", &path, &[], Some(body), None).await
        }
        "coordination.release" => {
            let path = format!("{}/leases/{}", c.agents_repo(args)?, seg(s("id").unwrap_or_default()));
            c.agents("DELETE", &path, &[], None, None).await.map(|_| json!({ "released": true }))
        }
        "coordination.activity" => {
            let mut q = Vec::new();
            if let Some(cur) = s("cursor") {
                q.push(("cursor", cur.to_owned()));
            }
            q.push(("wait", n("wait").unwrap_or(0).min(FEED_WAIT_MAX_S).to_string()));
            for p in strs(args, "paths") {
                q.push(("paths[]", p));
            }
            c.agents("GET", &format!("{}/activity", c.agents_repo(args)?), &q, None, None).await
        }
        "coordination.overlaps" => {
            let mut q = Vec::new();
            if let Some(r) = s("ref") {
                q.push(("ref", r.to_owned()));
            }
            if let Some(n) = n("pr") {
                q.push(("pr", n.to_string()));
            }
            for p in strs(args, "paths") {
                q.push(("paths[]", p));
            }
            c.agents("GET", &format!("{}/overlaps", c.agents_repo(args)?), &q, None, None).await
        }
        "provenance.why" => {
            let q = [
                ("ref", s("ref").unwrap_or_default().to_owned()),
                ("path", s("path").unwrap_or_default().to_owned()),
                ("line", num(args, "line")?.to_string()),
            ];
            c.agents("GET", &format!("{}/provenance", c.agents_repo(args)?), &q, None, None).await
        }
        _ => unreachable!("every listed tool is matched"),
    }
}

struct Caller<'a, E> {
    tok: &'a McpToken,
    edge: &'a E,
}

impl<E: Edge> Caller<'_, E> {
    /// `/api/v3/repos/{owner}/{repo}` for a repository of the grant, or 404.
    fn repo(&self, args: &Value) -> Result<String, ToolError> {
        Ok(format!("/api/v3/repos/{}", self.owned(args)?))
    }

    fn agents_repo(&self, args: &Value) -> Result<String, ToolError> {
        Ok(format!("/api/graftyard/v1/repos/{}", self.owned(args)?))
    }

    fn owned(&self, args: &Value) -> Result<String, ToolError> {
        let o = args.get("owner").and_then(Value::as_str).unwrap_or_default();
        let r = args.get("repo").and_then(Value::as_str).unwrap_or_default();
        let full = format!("{o}/{r}");
        if !self.tok.repos.iter().any(|g| g.eq_ignore_ascii_case(&full)) {
            return Err(ToolError::new(404, "repository not in this grant"));
        }
        Ok(format!("{}/{}", seg(o), seg(r)))
    }

    fn headers(&self, accept: &str) -> Vec<(&'static str, String)> {
        vec![("authorization", format!("Bearer {}", self.tok.upstream)), ("accept", accept.to_owned())]
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        mut h: Vec<(&'static str, String)>,
        body: Option<Value>,
    ) -> Result<(Answer, Value), ToolError> {
        let bytes = body.map(|b| {
            h.push(("content-type", "application/json".into()));
            b.to_string().into_bytes()
        });
        let a = self.edge.send(method, path, &h, bytes).await.map_err(|why| ToolError::new(502, why))?;
        let v: Value =
            if a.body.is_empty() { Value::Null } else { serde_json::from_slice(&a.body).unwrap_or(Value::Null) };
        if !(200..300).contains(&a.status) {
            let msg =
                v["message"].as_str().map(str::to_owned).unwrap_or_else(|| format!("the host answered {}", a.status));
            return Err(ToolError::of(&a, msg));
        }
        Ok((a, v))
    }

    async fn get(&self, path: &str, q: &[(&str, String)]) -> Result<Value, ToolError> {
        Ok(self.send("GET", &with_query(path, q), self.headers(AGENT_MEDIA_TYPE), None).await?.1)
    }

    /// A lease or feed call. The session header names the caller's task when it gives one.
    async fn agents(
        &self,
        method: &str,
        path: &str,
        q: &[(&str, String)],
        body: Option<Value>,
        session: Option<&str>,
    ) -> Result<Value, ToolError> {
        let mut h = self.headers("application/json");
        if let Some(s) = session {
            h.push(("x-graftyard-agent-session", s.to_owned()));
        }
        Ok(self.send(method, &with_query(path, q), h, body).await?.1)
    }

    /// One GraphQL mutation: its payload, or the first error as a `ToolError` (404 for an unresolved node, 422
    /// otherwise, GraphQL's own answer being 200).
    async fn mutate(&self, field: &str, select: &str, input: Value) -> Result<Value, ToolError> {
        let doc = format!("mutation($input: {}Input!) {{ {field}(input: $input) {{ {select} }} }}", pascal(field));
        let body = json!({ "query": doc, "variables": { "input": input } });
        let (a, v) = self.send("POST", "/api/graphql", self.headers("application/json"), Some(body)).await?;
        // A refused credential answers `{message, status}` in a 200, as GitHub's GraphQL does.
        if let Some(st) = v["status"].as_str().and_then(|s| s.parse::<u16>().ok()) {
            let msg = v["message"].as_str().unwrap_or("the mutation was refused").to_owned();
            return Err(ToolError::of(&Answer { status: st, ..a }, msg));
        }
        if let Some(e) = v["errors"].as_array().and_then(|e| e.first()) {
            let status = match e["type"].as_str() {
                Some("NOT_FOUND") => 404,
                Some("FORBIDDEN") => 403,
                _ => 422,
            };
            let msg = e["message"].as_str().unwrap_or("the mutation was refused").to_owned();
            return Err(ToolError::of(&Answer { status, ..a }, msg));
        }
        Ok(v["data"][field].clone())
    }

    /// The repository's node id, from `GET /repos/{o}/{r}`.
    async fn node(&self, args: &Value) -> Result<Value, ToolError> {
        let v = self.send("GET", &self.repo(args)?, self.headers("application/json"), None).await?.1;
        Ok(v["node_id"].clone())
    }

    /// An issue's or pull request's node id. The edge serves no single-issue REST read, so the pull request route
    /// answers for a pull request and the issue list's `number` for an issue.
    async fn item_node(&self, args: &Value) -> Result<Value, ToolError> {
        let n = num(args, "number")?;
        let repo = self.repo(args)?;
        match self.send("GET", &format!("{repo}/pulls/{n}"), self.headers("application/json"), None).await {
            Ok((_, v)) => return Ok(v["node_id"].clone()),
            Err(e) if e.status != 404 => return Err(e),
            Err(_) => {}
        }
        let mut page = 1;
        loop {
            let q = [("state", "all".to_owned()), ("per_page", "100".to_owned()), ("page", page.to_string())];
            let path = with_query(&format!("{repo}/issues"), &q);
            let list = self.send("GET", &path, self.headers("application/json"), None).await?.1;
            let items = list.as_array().cloned().unwrap_or_default();
            if let Some(it) = items.iter().find(|i| i["number"].as_u64() == Some(n)) {
                return Ok(it["node_id"].clone());
            }
            if items.len() < 100 || page >= 10 {
                return Err(ToolError::new(404, format!("no issue {n}")));
            }
            page += 1;
        }
    }
}

/// `v` with each trust-viewed text field replaced by its view's `visible`: `body` on issues, pull requests, comments,
/// and reviews, `commit.message` on commits, and `output.title`, `.summary`, and `.text` on check runs, which the view
/// joins into one `visible`. A field whose object carries no `graftyard_trust` is dropped rather than passed through,
/// so the model never reads text the host did not view.
pub fn trusted(mut v: Value) -> Value {
    fn one(m: &mut Map<String, Value>) {
        let view = m.remove("graftyard_trust");
        let visible = view.as_ref().and_then(|t| t["visible"].as_str()).map(str::to_owned);
        m.remove("body_html");
        m.remove("body_text");
        if m.contains_key("body") {
            m.insert("body".into(), visible.clone().map_or(Value::Null, Value::String));
        }
        if let Some(Value::Object(c)) = m.get_mut("commit")
            && c.contains_key("message")
        {
            c.insert("message".into(), visible.clone().map_or(Value::Null, Value::String));
        }
        if let Some(Value::Object(o)) = m.get_mut("output") {
            o.remove("title");
            o.remove("summary");
            o.remove("text");
            o.insert("visible".into(), visible.map_or(Value::Null, Value::String));
        }
        if let Some(t) = view {
            m.insert("trust".into(), json!({ "hidden": t["hidden"], "author": t["author"] }));
        }
    }
    match &mut v {
        Value::Array(xs) => xs.iter_mut().filter_map(Value::as_object_mut).for_each(one),
        Value::Object(m) if m.contains_key("check_runs") => {
            if let Some(Value::Array(xs)) = m.get_mut("check_runs") {
                xs.iter_mut().filter_map(Value::as_object_mut).for_each(one);
            }
        }
        Value::Object(m) => one(m),
        _ => {}
    }
    v
}

fn num(args: &Value, k: &str) -> Result<u64, ToolError> {
    args.get(k).and_then(Value::as_u64).ok_or_else(|| ToolError::new(422, format!("{k} must be a positive integer")))
}

fn strs(args: &Value, k: &str) -> Vec<String> {
    args.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// One path segment, percent-encoded so an argument cannot add segments or a query.
fn seg(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

fn with_query(path: &str, q: &[(&str, String)]) -> String {
    if q.is_empty() {
        return path.to_owned();
    }
    let mut s = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in q {
        s.append_pair(k, v);
    }
    format!("{path}?{}", s.finish())
}

fn pascal(field: &str) -> String {
    let mut c = field.chars();
    c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
}
