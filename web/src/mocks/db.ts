import type { Schemas } from "@/api/client";

// One in-memory store behind every mock handler. Every record is typed by the GHES 3.22
// schema it is served as, so a fixture that drifts from the wire contract fails typecheck.

type SimpleUser = Schemas["simple-user"];
type Repository = Schemas["repository"];
type FullRepository = Schemas["full-repository"];
type Issue = Schemas["issue"];
type IssueComment = Schemas["issue-comment"];
type PullRequest = Schemas["pull-request"] & { labels: Label[] };
type DiffEntry = Schemas["diff-entry"];
type CheckRun = Schemas["check-run"];
// The pulls list schema (pull-request-simple) requires a non-null description, the strictest of the three.
type Label = Schemas["pull-request-simple"]["labels"][number];

const HOST = "https://ghes.graftyard.test";
const API = `${HOST}/api/v3`;

let nextId = 1000;
const id = () => nextId++;

// A stable 40-hex digest so shas and node ids look real and never change between loads.
function sha(seed: string) {
  let h = 0x811c9dc5;
  let out = "";
  for (let round = 0; out.length < 40; round++) {
    for (const ch of `${seed}:${round}`) h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193);
    out += (h >>> 0).toString(16).padStart(8, "0");
  }
  return out.slice(0, 40);
}

function avatar(login: string) {
  const hue = parseInt(sha(login).slice(0, 4), 16) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect width="40" height="40" fill="hsl(${hue} 60% 34%)"/><text x="20" y="26" font-family="sans-serif" font-size="16" font-weight="600" fill="white" text-anchor="middle">${login.slice(0, 2).toUpperCase()}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

function user(login: string, name: string, type = "User"): SimpleUser {
  const n = id();
  return {
    name,
    login,
    id: n,
    node_id: `U_${sha(login).slice(0, 12)}`,
    avatar_url: avatar(login),
    gravatar_id: "",
    url: `${API}/users/${login}`,
    html_url: `${HOST}/${login}`,
    followers_url: `${API}/users/${login}/followers`,
    following_url: `${API}/users/${login}/following{/other_user}`,
    gists_url: `${API}/users/${login}/gists{/gist_id}`,
    starred_url: `${API}/users/${login}/starred{/owner}{/repo}`,
    subscriptions_url: `${API}/users/${login}/subscriptions`,
    organizations_url: `${API}/users/${login}/orgs`,
    repos_url: `${API}/users/${login}/repos`,
    events_url: `${API}/users/${login}/events{/privacy}`,
    received_events_url: `${API}/users/${login}/received_events`,
    type,
    site_admin: false,
  };
}

const users = {
  mira: user("mira", "Mira Okafor"),
  tomas: user("tomas", "Tomás Ferreira"),
  lin: user("lin", "Lin Zhao"),
  devi: user("devi", "Devi Raman"),
  org: user("graftyard", "Graftyard", "Organization"),
};
const actionsApp: CheckRun["app"] = {
  id: 15368,
  slug: "github-actions",
  node_id: "MDM6QXBwMTUzNjg=",
  owner: users.org,
  name: "GitHub Actions",
  description: "Automate your workflow from idea to production",
  external_url: `${HOST}/features/actions`,
  html_url: `${HOST}/apps/github-actions`,
  created_at: "2025-01-10T00:00:00Z",
  updated_at: "2026-10-01T00:00:00Z",
  permissions: { checks: "write", contents: "read", metadata: "read" },
  events: ["check_run", "pull_request", "push"],
};

export const me: Schemas["private-user"] = {
  ...users.mira,
  user_view_type: "private",
  name: "Mira Okafor",
  company: "Graftyard",
  blog: null,
  location: "Lisbon",
  email: "mira@graftyard.test",
  hireable: null,
  bio: "Edge and storage.",
  public_repos: 3,
  public_gists: 0,
  followers: 12,
  following: 4,
  created_at: "2025-03-02T09:00:00Z",
  updated_at: "2026-11-01T09:00:00Z",
  private_gists: 0,
  total_private_repos: 1,
  owned_private_repos: 0,
  disk_usage: 20480,
  collaborators: 0,
  two_factor_authentication: true,
};

function repository(
  name: string,
  description: string,
  visibility: "public" | "internal" | "private",
  extra: { language: string; stars: number; topics: string[]; pushed: string },
): Repository {
  const full = `graftyard/${name}`;
  const base = `${API}/repos/${full}`;
  return {
    id: id(),
    node_id: `R_${sha(full).slice(0, 12)}`,
    name,
    full_name: full,
    license: {
      key: "apache-2.0",
      name: "Apache License 2.0",
      url: `${API}/licenses/apache-2.0`,
      spdx_id: "Apache-2.0",
      node_id: "MDc6TGljZW5zZTI=",
    },
    forks: 3,
    permissions: { admin: true, maintain: true, push: true, triage: true, pull: true },
    owner: users.org,
    private: visibility !== "public",
    html_url: `${HOST}/${full}`,
    description,
    fork: false,
    url: base,
    archive_url: `${base}/{archive_format}{/ref}`,
    assignees_url: `${base}/assignees{/user}`,
    blobs_url: `${base}/git/blobs{/sha}`,
    branches_url: `${base}/branches{/branch}`,
    collaborators_url: `${base}/collaborators{/collaborator}`,
    comments_url: `${base}/comments{/number}`,
    commits_url: `${base}/commits{/sha}`,
    compare_url: `${base}/compare/{base}...{head}`,
    contents_url: `${base}/contents/{+path}`,
    contributors_url: `${base}/contributors`,
    deployments_url: `${base}/deployments`,
    downloads_url: `${base}/downloads`,
    events_url: `${base}/events`,
    forks_url: `${base}/forks`,
    git_commits_url: `${base}/git/commits{/sha}`,
    git_refs_url: `${base}/git/refs{/sha}`,
    git_tags_url: `${base}/git/tags{/sha}`,
    git_url: `git://ghes.graftyard.test/${full}.git`,
    issue_comment_url: `${base}/issues/comments{/number}`,
    issue_events_url: `${base}/issues/events{/number}`,
    issues_url: `${base}/issues{/number}`,
    keys_url: `${base}/keys{/key_id}`,
    labels_url: `${base}/labels{/name}`,
    languages_url: `${base}/languages`,
    merges_url: `${base}/merges`,
    milestones_url: `${base}/milestones{/number}`,
    notifications_url: `${base}/notifications{?since,all,participating}`,
    pulls_url: `${base}/pulls{/number}`,
    releases_url: `${base}/releases{/id}`,
    ssh_url: `git@ghes.graftyard.test:${full}.git`,
    stargazers_url: `${base}/stargazers`,
    statuses_url: `${base}/statuses/{sha}`,
    subscribers_url: `${base}/subscribers`,
    subscription_url: `${base}/subscription`,
    tags_url: `${base}/tags`,
    teams_url: `${base}/teams`,
    trees_url: `${base}/git/trees{/sha}`,
    clone_url: `${HOST}/${full}.git`,
    mirror_url: null,
    hooks_url: `${base}/hooks`,
    svn_url: `${HOST}/${full}`,
    homepage: null,
    language: extra.language,
    forks_count: 3,
    stargazers_count: extra.stars,
    watchers_count: extra.stars,
    size: 4096,
    default_branch: "main",
    open_issues_count: 0,
    is_template: false,
    topics: extra.topics,
    has_issues: true,
    has_projects: false,
    has_wiki: false,
    has_pages: false,
    has_downloads: true,
    has_discussions: false,
    has_pull_requests: true,
    archived: false,
    disabled: false,
    visibility,
    pushed_at: extra.pushed,
    created_at: "2025-03-02T09:00:00Z",
    updated_at: extra.pushed,
    allow_rebase_merge: true,
    allow_squash_merge: true,
    allow_auto_merge: false,
    delete_branch_on_merge: true,
    allow_update_branch: true,
    use_squash_pr_title_as_default: true,
    allow_merge_commit: false,
    web_commit_signoff_required: false,
    open_issues: 0,
    watchers: extra.stars,
  };
}

function fullRepository(r: Repository): FullRepository {
  return {
    ...r,
    pushed_at: r.pushed_at ?? r.created_at ?? "",
    created_at: r.created_at ?? "",
    updated_at: r.updated_at ?? "",
    subscribers_count: 6,
    network_count: 3,
    organization: users.org,
    anonymous_access_enabled: false,
  };
}

const LABELS: Record<string, [color: string, description: string]> = {
  bug: ["d73a4a", "Something is not working"],
  edge: ["1d76db", "The Rust edge Worker"],
  wire: ["5319e7", "GHES wire compatibility"],
  perf: ["fbca04", "Latency or throughput"],
  docs: ["0075ca", "Documentation"],
  "good first issue": ["7057ff", "Good for newcomers"],
  ui: ["0e8a16", "The web app"],
};

function label(repo: Repository, name: string): Label {
  const [color, description] = LABELS[name];
  return {
    id: parseInt(sha(name).slice(0, 7), 16),
    node_id: `LA_${sha(name).slice(0, 12)}`,
    url: `${repo.url}/labels/${encodeURIComponent(name)}`,
    name,
    description,
    color,
    default: name === "bug" || name === "docs",
    archived_by: null,
  };
}

type IssueSeed = {
  number: number;
  title: string;
  body: string;
  author: SimpleUser;
  labels?: string[];
  closed?: string;
  created: string;
  updated: string;
};

function issue(repo: Repository, s: IssueSeed): Issue {
  const url = `${repo.url}/issues/${s.number}`;
  return {
    id: id(),
    node_id: `I_${sha(`${repo.full_name}#${s.number}`).slice(0, 12)}`,
    url,
    repository_url: repo.url,
    labels_url: `${url}/labels{/name}`,
    comments_url: `${url}/comments`,
    events_url: `${url}/events`,
    html_url: `${repo.html_url}/issues/${s.number}`,
    number: s.number,
    state: s.closed ? "closed" : "open",
    state_reason: s.closed ? "completed" : null,
    title: s.title,
    body: s.body,
    user: s.author,
    labels: (s.labels ?? []).map((n) => label(repo, n)),
    assignee: null,
    assignees: [],
    milestone: null,
    locked: false,
    comments: 0,
    closed_at: s.closed ?? null,
    created_at: s.created,
    updated_at: s.updated,
    author_association: s.author.login === "mira" ? "OWNER" : "MEMBER",
  };
}

type PullSeed = IssueSeed & {
  head: string;
  draft?: boolean;
  merged?: string;
  files: DiffEntry[];
};

function pullRequest(repo: Repository, s: PullSeed): PullRequest {
  const url = `${repo.url}/pulls/${s.number}`;
  const headSha = sha(`${repo.full_name}:${s.head}`);
  const additions = s.files.reduce((n, f) => n + f.additions, 0);
  const deletions = s.files.reduce((n, f) => n + f.deletions, 0);
  const link = (href: string) => ({ href });
  return {
    url,
    id: id(),
    node_id: `PR_${sha(url).slice(0, 12)}`,
    html_url: `${repo.html_url}/pull/${s.number}`,
    diff_url: `${repo.html_url}/pull/${s.number}.diff`,
    patch_url: `${repo.html_url}/pull/${s.number}.patch`,
    issue_url: `${repo.url}/issues/${s.number}`,
    commits_url: `${url}/commits`,
    review_comments_url: `${url}/comments`,
    review_comment_url: `${repo.url}/pulls/comments{/number}`,
    comments_url: `${repo.url}/issues/${s.number}/comments`,
    statuses_url: `${repo.url}/statuses/${headSha}`,
    number: s.number,
    state: s.closed ? "closed" : "open",
    locked: false,
    title: s.title,
    user: s.author,
    body: s.body,
    labels: (s.labels ?? []).map((n) => label(repo, n)),
    milestone: null,
    created_at: s.created,
    updated_at: s.updated,
    closed_at: s.closed ?? null,
    merged_at: s.merged ?? null,
    merge_commit_sha: s.merged ? sha(`${repo.full_name}:merge:${s.number}`) : null,
    assignee: null,
    assignees: [],
    requested_reviewers: [],
    requested_teams: [],
    head: { label: `graftyard:${s.head}`, ref: s.head, repo, sha: headSha, user: users.org },
    base: {
      label: "graftyard:main",
      ref: "main",
      repo,
      sha: sha(`${repo.full_name}:main`),
      user: users.org,
    },
    _links: {
      comments: link(`${repo.url}/issues/${s.number}/comments`),
      commits: link(`${url}/commits`),
      statuses: link(`${repo.url}/statuses/${headSha}`),
      html: link(`${repo.html_url}/pull/${s.number}`),
      issue: link(`${repo.url}/issues/${s.number}`),
      review_comments: link(`${url}/comments`),
      review_comment: link(`${repo.url}/pulls/comments{/number}`),
      self: link(url),
    },
    author_association: s.author.login === "mira" ? "OWNER" : "MEMBER",
    auto_merge: null,
    draft: s.draft ?? false,
    merged: Boolean(s.merged),
    mergeable: s.closed ? null : true,
    rebaseable: s.closed ? null : true,
    mergeable_state: s.draft ? "draft" : s.closed ? "unknown" : "clean",
    merged_by: s.merged ? users.mira : null,
    comments: 0,
    review_comments: 0,
    maintainer_can_modify: false,
    commits: Math.max(1, s.files.length - 1),
    additions,
    deletions,
    changed_files: s.files.length,
  };
}

// GET /pulls answers pull-request-simple: the full shape without merge and size fields.
function simplePull(pr: PullRequest): Schemas["pull-request-simple"] {
  const {
    merged,
    mergeable,
    rebaseable,
    mergeable_state,
    merged_by,
    comments,
    review_comments,
    maintainer_can_modify,
    commits,
    additions,
    deletions,
    changed_files,
    requested_teams,
    ...rest
  } = pr;
  return { ...rest, requested_teams: [] };
}

// The issues list returns pull requests too, marked with a pull_request key.
function issueForPull(repo: Repository, s: PullSeed, pr: PullRequest): Issue {
  return {
    ...issue(repo, s),
    id: pr.id,
    html_url: pr.html_url,
    draft: pr.draft,
    pull_request: {
      merged_at: pr.merged_at,
      diff_url: pr.diff_url,
      html_url: pr.html_url,
      patch_url: pr.patch_url,
      url: pr.url,
    },
  };
}

function file(
  repo: Repository,
  head: string,
  filename: string,
  status: DiffEntry["status"],
  patch: string,
): DiffEntry {
  const lines = patch.split("\n");
  const additions = lines.filter((l) => l.startsWith("+")).length;
  const deletions = lines.filter((l) => l.startsWith("-")).length;
  const blob = sha(`${repo.full_name}:${head}:${filename}`);
  return {
    sha: blob,
    filename,
    status,
    additions,
    deletions,
    changes: additions + deletions,
    blob_url: `${repo.html_url}/blob/${blob}/${filename}`,
    raw_url: `${repo.html_url}/raw/${blob}/${filename}`,
    contents_url: `${repo.url}/contents/${filename}?ref=${blob}`,
    patch,
  };
}

function comment(
  repo: Repository,
  number: number,
  author: SimpleUser,
  body: string,
  at: string,
): IssueComment {
  const n = id();
  return {
    id: n,
    node_id: `IC_${sha(`${n}`).slice(0, 12)}`,
    url: `${repo.url}/issues/comments/${n}`,
    html_url: `${repo.html_url}/issues/${number}#issuecomment-${n}`,
    issue_url: `${repo.url}/issues/${number}`,
    body,
    user: author,
    created_at: at,
    updated_at: at,
    author_association: author.login === "mira" ? "OWNER" : "MEMBER",
  };
}

function checkRun(
  repo: Repository,
  headSha: string,
  name: string,
  status: CheckRun["status"],
  conclusion: CheckRun["conclusion"],
  started: string,
  completed: string | null,
  summary: string,
): CheckRun {
  const n = id();
  return {
    id: n,
    head_sha: headSha,
    node_id: `CR_${sha(`${n}`).slice(0, 12)}`,
    external_id: `${n}`,
    url: `${repo.url}/check-runs/${n}`,
    html_url: `${repo.html_url}/actions/runs/${n}`,
    details_url: `${repo.html_url}/actions/runs/${n}`,
    status,
    conclusion,
    started_at: started,
    completed_at: completed,
    output: {
      title: summary,
      summary,
      text: null,
      annotations_count: 0,
      annotations_url: `${repo.url}/check-runs/${n}/annotations`,
    },
    name,
    check_suite: { id: n + 1 },
    app: actionsApp,
    pull_requests: [],
  };
}

// ---- Seed data ----

const edge = repository(
  "edge",
  "The Rust edge Worker: REST and GraphQL wire compatibility with GHES 3.22.",
  "public",
  {
    language: "Rust",
    stars: 214,
    topics: ["cloudflare-workers", "rust", "ghes"],
    pushed: "2026-11-08T10:12:00Z",
  },
);
const web = repository("web", "The Graftyard web app.", "internal", {
  language: "TypeScript",
  stars: 38,
  topics: ["react", "tanstack"],
  pushed: "2026-11-07T17:40:00Z",
});
const raft = repository("raft", "Log, lease, transfer, and learners for each group.", "private", {
  language: "Rust",
  stars: 91,
  topics: ["raft", "consensus"],
  pushed: "2026-11-05T08:03:00Z",
});

const edgeIssues: IssueSeed[] = [
  {
    number: 2,
    title: "GET /repos/{owner}/{repo}/issues omits Link header on the last page",
    body: 'gh paginates with the `Link` header. On the last page we send none, which is right, but on page 1 of 1 we also send `rel="last"` pointing at page 1. GHES 3.22 omits it.\n\nRepro: `gh issue list -L 200` against a repo with 30 issues.',
    author: users.tomas,
    labels: ["bug", "wire"],
    created: "2026-10-28T09:14:00Z",
    updated: "2026-11-06T15:02:00Z",
  },
  {
    number: 3,
    title: "Check runs list returns 404 for a branch name ref",
    body: "`GET /repos/{owner}/{repo}/commits/main/check-runs` answers 404. GHES resolves `ref` as a sha, a branch, or a tag.",
    author: users.lin,
    labels: ["bug", "edge"],
    created: "2026-10-29T11:00:00Z",
    updated: "2026-11-07T08:21:00Z",
  },
  {
    number: 4,
    title: "Document the retry budget for idempotent reads",
    body: "The web app and the CLI both retry GETs. Write down the budget (2 retries, full jitter) in one place.",
    author: users.mira,
    labels: ["docs"],
    created: "2026-10-30T13:30:00Z",
    updated: "2026-11-01T10:00:00Z",
  },
  {
    number: 6,
    title: "p99 for issue list is 180 ms on a cold group",
    body: "Measured from three regions. The fold read dominates. A learner read-index should halve it.",
    author: users.devi,
    labels: ["perf", "edge"],
    created: "2026-10-31T07:45:00Z",
    updated: "2026-11-08T09:40:00Z",
  },
  {
    number: 7,
    title: "Label colours render without the leading hash in the web app",
    body: "GHES sends `color` without `#`. The badge must add it.",
    author: users.lin,
    labels: ["ui", "good first issue"],
    closed: "2026-11-03T12:00:00Z",
    created: "2026-11-01T09:00:00Z",
    updated: "2026-11-03T12:00:00Z",
  },
  {
    number: 8,
    title: "POST /issues ignores `labels` given as objects",
    body: "The schema allows `labels: [{ name }]`. We only accept strings.",
    author: users.tomas,
    labels: ["wire"],
    created: "2026-11-01T16:20:00Z",
    updated: "2026-11-02T08:00:00Z",
  },
  {
    number: 10,
    title: "Support `If-None-Match` on repository GET",
    body: "Clients send the ETag from the previous response. Answer 304 when it matches.",
    author: users.devi,
    labels: ["perf", "wire"],
    created: "2026-11-02T10:10:00Z",
    updated: "2026-11-04T14:00:00Z",
  },
  {
    number: 11,
    title: "Rate limit headers missing on 503",
    body: "When a region fails we answer 503 without `x-ratelimit-*`. Octokit's throttling plugin then backs off forever.",
    author: users.mira,
    labels: ["bug", "edge"],
    closed: "2026-11-05T18:30:00Z",
    created: "2026-11-03T08:00:00Z",
    updated: "2026-11-05T18:30:00Z",
  },
  {
    number: 12,
    title: "Add a quickstart for running the mocked web app",
    body: "`npm run dev` in `web/` with MSW. Explain `?fail=`.",
    author: users.lin,
    labels: ["docs", "ui", "good first issue"],
    created: "2026-11-04T09:00:00Z",
    updated: "2026-11-04T09:00:00Z",
  },
  {
    number: 13,
    title: "Timeline events for label changes",
    body: "GHES returns `labeled` and `unlabeled` events on the issue timeline. We return none yet.",
    author: users.tomas,
    labels: ["wire"],
    closed: "2026-11-06T10:00:00Z",
    created: "2026-11-04T15:00:00Z",
    updated: "2026-11-06T10:00:00Z",
  },
  {
    number: 15,
    title: "Search box should match issue numbers",
    body: "Typing `#12` in the issue filter should find issue 12.",
    author: users.devi,
    labels: ["ui"],
    created: "2026-11-06T11:11:00Z",
    updated: "2026-11-07T16:45:00Z",
  },
  {
    number: 16,
    title: "Edge panics on an empty `per_page`",
    body: "`?per_page=` (empty) panics in the query parser. GHES treats it as the default, 30.",
    author: users.tomas,
    labels: ["bug"],
    created: "2026-11-07T19:02:00Z",
    updated: "2026-11-08T07:30:00Z",
  },
];

const edgePulls: PullSeed[] = [
  {
    number: 1,
    title: "Retry only idempotent reads at the edge client",
    body: "The edge's outbound client retried every request on a 503, POSTs included. This limits retries to GET and HEAD, caps them at 2, and adds full jitter.\n\nCloses #4.",
    author: users.mira,
    labels: ["edge"],
    head: "mira/idempotent-retry",
    created: "2026-11-05T09:00:00Z",
    updated: "2026-11-08T10:12:00Z",
    files: [
      file(
        edge,
        "mira/idempotent-retry",
        "crates/wire/src/retry.rs",
        "modified",
        `@@ -1,18 +1,27 @@
 use std::time::Duration;
 
-pub const MAX_RETRIES: u32 = 5;
+pub const MAX_RETRIES: u32 = 2;
+const BASE_MS: u64 = 300;
+const CAP_MS: u64 = 4_000;
 
-pub fn should_retry(status: u16) -> bool {
-    status >= 500
+/// Only reads are safe to send twice.
+pub fn should_retry(method: &Method, status: u16, attempt: u32) -> bool {
+    method.is_idempotent_read() && status >= 500 && attempt < MAX_RETRIES
 }
 
-pub fn backoff(attempt: u32) -> Duration {
-    Duration::from_millis(100 * 2u64.pow(attempt))
+/// Full jitter: a uniform delay below an exponential cap.
+pub fn backoff(attempt: u32, rand: f64) -> Duration {
+    let cap = (BASE_MS << attempt).min(CAP_MS);
+    Duration::from_millis((rand * cap as f64) as u64)
 }
 
 #[cfg(test)]
 mod tests {
     use super::*;
 
     #[test]
-    fn retries_server_errors() {
-        assert!(should_retry(503));
+    fn never_retries_a_post() {
+        assert!(!should_retry(&Method::Post, 503, 0));
+        assert!(should_retry(&Method::Get, 503, 0));
+        assert!(!should_retry(&Method::Get, 503, 2));
     }
 }`,
      ),
      file(
        edge,
        "mira/idempotent-retry",
        "crates/wire/src/method.rs",
        "added",
        `@@ -0,0 +1,14 @@
+#[derive(Clone, Copy, Debug, PartialEq, Eq)]
+pub enum Method {
+    Get,
+    Head,
+    Post,
+    Patch,
+    Delete,
+}
+
+impl Method {
+    pub fn is_idempotent_read(&self) -> bool {
+        matches!(self, Method::Get | Method::Head)
+    }
+}`,
      ),
      file(
        edge,
        "mira/idempotent-retry",
        "crates/wire/src/lib.rs",
        "modified",
        `@@ -3,6 +3,7 @@ pub mod error;
 pub mod headers;
 pub mod link;
+pub mod method;
 pub mod pagination;
 pub mod retry;
 
@@ -21,7 +22,7 @@ pub fn client() -> Client {
     Client::builder()
         .timeout(Duration::from_secs(10))
-        .retry(retry::should_retry)
+        .retry(|req, res, n| retry::should_retry(req.method(), res.status(), n))
         .build()
 }`,
      ),
    ],
  },
  {
    number: 5,
    title: "Send rate limit headers on every error response",
    body: "Fixes #11. Adds `x-ratelimit-limit`, `-remaining`, `-used`, and `-reset` to 4xx and 5xx responses.",
    author: users.devi,
    labels: ["edge", "wire"],
    head: "devi/ratelimit-on-errors",
    merged: "2026-11-05T18:30:00Z",
    closed: "2026-11-05T18:30:00Z",
    created: "2026-11-04T10:00:00Z",
    updated: "2026-11-05T18:30:00Z",
    files: [
      file(
        edge,
        "devi/ratelimit-on-errors",
        "crates/wire/src/error.rs",
        "modified",
        `@@ -40,9 +40,11 @@ impl ErrorResponse {
     pub fn render(&self, limits: &RateLimit) -> Response {
         let mut res = Response::json(&self.body).with_status(self.status);
-        if self.status < 400 {
-            limits.apply(res.headers_mut());
-        }
+        // GHES sends the rate limit headers on errors too, and
+        // Octokit's throttling plugin reads them from 403 and 429.
+        limits.apply(res.headers_mut());
         res
     }
 }`,
      ),
    ],
  },
  {
    number: 9,
    title: "Draft: learner read-index for hot issue lists",
    body: "Work in progress for #6. Reads the issue list from a learner when the leader is cold.",
    author: users.lin,
    labels: ["perf"],
    head: "lin/learner-reads",
    draft: true,
    created: "2026-11-06T14:00:00Z",
    updated: "2026-11-07T21:10:00Z",
    files: [
      file(
        edge,
        "lin/learner-reads",
        "crates/raft/src/read.rs",
        "modified",
        `@@ -12,6 +12,15 @@ pub enum ReadPath {
     Leader,
     Lease,
+    /// A learner that has applied at least the leader's commit index.
+    Learner { min_index: u64 },
 }
 
+pub fn pick(leader_warm: bool, learner_index: u64, commit: u64) -> ReadPath {
+    if !leader_warm && learner_index >= commit {
+        return ReadPath::Learner { min_index: commit };
+    }
+    ReadPath::Lease
+}
+
 impl ReadPath {`,
      ),
    ],
  },
  {
    number: 14,
    title: "Accept label objects on issue create",
    body: "Superseded by a wire-level fix. Closing.",
    author: users.tomas,
    labels: ["wire"],
    head: "tomas/label-objects",
    closed: "2026-11-07T09:00:00Z",
    created: "2026-11-06T08:00:00Z",
    updated: "2026-11-07T09:00:00Z",
    files: [
      file(
        edge,
        "tomas/label-objects",
        "crates/api/src/rest/issues.rs",
        "modified",
        `@@ -88,7 +88,10 @@ fn parse_create(body: &Value) -> Result<CreateIssue, Invalid> {
-    let labels = body["labels"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect());
+    let labels = body["labels"].as_array().map(|a| {
+        a.iter()
+            .filter_map(|l| l.as_str().or_else(|| l["name"].as_str()))
+            .collect()
+    });
     Ok(CreateIssue { title, body: text, labels })
 }`,
      ),
    ],
  },
];

type RepoState = {
  repo: Repository;
  issues: Issue[];
  pulls: PullRequest[];
  files: Map<number, DiffEntry[]>;
  comments: Map<number, IssueComment[]>;
};

function seedRepo(repo: Repository, issues: IssueSeed[], pulls: PullSeed[]): RepoState {
  const state: RepoState = { repo, issues: [], pulls: [], files: new Map(), comments: new Map() };
  for (const s of issues) state.issues.push(issue(repo, s));
  for (const s of pulls) {
    const pr = pullRequest(repo, s);
    state.pulls.push(pr);
    state.issues.push(issueForPull(repo, s, pr));
    state.files.set(s.number, s.files);
  }
  return state;
}

const repos = new Map<string, RepoState>([
  ["graftyard/edge", seedRepo(edge, edgeIssues, edgePulls)],
  [
    "graftyard/web",
    seedRepo(
      web,
      [
        {
          number: 1,
          title: "Dark mode contrast on diff deletions",
          body: "The red on dark grey fails WCAG AA.",
          author: users.lin,
          labels: ["ui", "bug"],
          created: "2026-11-06T09:00:00Z",
          updated: "2026-11-07T09:00:00Z",
        },
        {
          number: 2,
          title: "Keyboard shortcut to jump between files",
          body: "`j` and `k` like the GHES UI.",
          author: users.devi,
          labels: ["ui"],
          created: "2026-11-07T10:00:00Z",
          updated: "2026-11-07T10:00:00Z",
        },
      ],
      [],
    ),
  ],
  [
    "graftyard/raft",
    seedRepo(
      raft,
      [
        {
          number: 1,
          title: "Pre-vote stalls when two members restart together",
          body: "Seen once in the chaos run.",
          author: users.tomas,
          labels: ["bug"],
          created: "2026-11-02T09:00:00Z",
          updated: "2026-11-05T08:03:00Z",
        },
      ],
      [],
    ),
  ],
]);

function addComment(full: string, number: number, author: SimpleUser, body: string, at: string) {
  const state = repos.get(full)!;
  const list = state.comments.get(number) ?? [];
  list.push(comment(state.repo, number, author, body, at));
  state.comments.set(number, list);
  const target = state.issues.find((i) => i.number === number)!;
  target.comments = list.length;
  const pr = state.pulls.find((p) => p.number === number);
  if (pr) pr.comments = list.length;
}

addComment(
  "graftyard/edge",
  2,
  users.mira,
  "Confirmed against a GHES 3.22 appliance. Page 1 of 1 has no `Link` header at all.",
  "2026-10-28T12:00:00Z",
);
addComment(
  "graftyard/edge",
  2,
  users.tomas,
  "I'll take it. The fix is in `link.rs`, when `last == current`.",
  "2026-11-06T15:02:00Z",
);
addComment(
  "graftyard/edge",
  3,
  users.devi,
  "We parse `ref` as a sha only. Branch and tag lookup needs the refs fold.",
  "2026-11-07T08:21:00Z",
);
addComment(
  "graftyard/edge",
  6,
  users.lin,
  "Learner reads are drafted in #9.",
  "2026-11-08T09:40:00Z",
);
addComment("graftyard/edge", 11, users.devi, "Fixed by #5.", "2026-11-05T18:30:00Z");
addComment(
  "graftyard/edge",
  1,
  users.lin,
  "Should HEAD count as a read here? Octokit never sends it, but `git` does for `info/refs`.",
  "2026-11-06T10:00:00Z",
);
addComment(
  "graftyard/edge",
  1,
  users.mira,
  "Yes. HEAD has no body and is safe to repeat.",
  "2026-11-06T11:30:00Z",
);
addComment(
  "graftyard/edge",
  1,
  users.tomas,
  "The `rustfmt` check is red because of the long closure in `lib.rs`.",
  "2026-11-08T10:12:00Z",
);

const checkRuns = new Map<string, CheckRun[]>();
for (const state of repos.values()) {
  for (const pr of state.pulls) {
    const t = pr.created_at;
    const runs =
      pr.number === 1
        ? [
            checkRun(
              edge,
              pr.head.sha,
              "test (ubuntu-latest)",
              "completed",
              "success",
              t,
              "2026-11-08T10:20:00Z",
              "312 tests passed",
            ),
            checkRun(
              edge,
              pr.head.sha,
              "rustfmt",
              "completed",
              "failure",
              t,
              "2026-11-08T10:14:00Z",
              "crates/wire/src/lib.rs needs formatting",
            ),
            checkRun(
              edge,
              pr.head.sha,
              "byte-diff (GHES 3.22)",
              "in_progress",
              null,
              "2026-11-08T10:13:00Z",
              null,
              "Comparing 1,204 recorded responses",
            ),
            checkRun(
              edge,
              pr.head.sha,
              "clippy",
              "completed",
              "success",
              t,
              "2026-11-08T10:16:00Z",
              "No warnings",
            ),
          ]
        : pr.draft
          ? [
              checkRun(
                edge,
                pr.head.sha,
                "test (ubuntu-latest)",
                "queued",
                null,
                t,
                null,
                "Waiting for a runner",
              ),
            ]
          : [
              checkRun(
                edge,
                pr.head.sha,
                "test (ubuntu-latest)",
                "completed",
                pr.merged ? "success" : "cancelled",
                t,
                pr.updated_at,
                pr.merged ? "298 tests passed" : "Cancelled",
              ),
              checkRun(
                edge,
                pr.head.sha,
                "rustfmt",
                "completed",
                "success",
                t,
                pr.updated_at,
                "Formatted",
              ),
            ];
    checkRuns.set(pr.head.sha, runs);
  }
}

for (const state of repos.values()) {
  state.repo.open_issues_count = state.repo.open_issues = state.issues.filter(
    (i) => i.state === "open",
  ).length;
}

// ---- Store API the handlers use ----

export const db = {
  me,
  repos: () => [...repos.values()].map((s) => s.repo),
  repo: (owner: string, name: string) => {
    const state = repos.get(`${owner}/${name}`.toLowerCase());
    return state && fullRepository(state.repo);
  },
  state: (owner: string, name: string) => repos.get(`${owner}/${name}`.toLowerCase()),
  checkRuns: (headSha: string) => checkRuns.get(headSha) ?? [],
  simplePull,

  createIssue(state: RepoState, input: { title: string; body?: string; labels?: string[] }): Issue {
    const number = Math.max(0, ...state.issues.map((i) => i.number)) + 1;
    const now = new Date().toISOString();
    const created = issue(state.repo, {
      number,
      title: input.title,
      body: input.body ?? "",
      author: users.mira,
      labels: (input.labels ?? []).filter((l) => l in LABELS),
      created: now,
      updated: now,
    });
    state.issues.push(created);
    state.repo.open_issues_count = state.repo.open_issues = state.repo.open_issues + 1;
    return created;
  },

  createComment(state: RepoState, number: number, body: string): IssueComment {
    const now = new Date().toISOString();
    addComment(state.repo.full_name, number, users.mira, body, now);
    const target = state.issues.find((i) => i.number === number)!;
    target.updated_at = now;
    return state.comments.get(number)!.at(-1)!;
  },
};

export type { RepoState };
