import type { Schemas } from "@/api/client";

// The shared base every mock area builds on: ids, digests, the people, the org, and the
// repositories. Each area under ./areas keeps its own records and imports these. Every
// record is typed by the GHES 3.22 schema it is served as, so a fixture that drifts from
// the wire contract fails typecheck.

export type SimpleUser = Schemas["simple-user"];
export type Repository = Schemas["repository"];
type FullRepository = Schemas["full-repository"];

export const HOST = "https://ghes.graftyard.test";
export const API = `${HOST}/api/v3`;

let nextId = 1000;
export const id = () => nextId++;

// A stable 40-hex digest so shas and node ids look real and never change between loads.
export function sha(seed: string) {
  let h = 0x811c9dc5;
  let out = "";
  for (let round = 0; out.length < 40; round++) {
    for (const ch of `${seed}:${round}`) h = Math.imul(h ^ ch.charCodeAt(0), 0x01000193);
    out += (h >>> 0).toString(16).padStart(8, "0");
  }
  return out.slice(0, 40);
}

export function avatar(login: string) {
  const hue = parseInt(sha(login).slice(0, 4), 16) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect width="40" height="40" fill="hsl(${hue} 60% 34%)"/><text x="20" y="26" font-family="sans-serif" font-size="16" font-weight="600" fill="white" text-anchor="middle">${login.slice(0, 2).toUpperCase()}</text></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

export function user(login: string, name: string, type = "User"): SimpleUser {
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

export const users = {
  mira: user("mira", "Mira Okafor"),
  tomas: user("tomas", "Tomás Ferreira"),
  lin: user("lin", "Lin Zhao"),
  devi: user("devi", "Devi Raman"),
  org: user("graftyard", "Graftyard", "Organization"),
};
export const actionsApp: Schemas["integration"] = {
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
  // The viewer administers the site, so the admin console is reachable.
  site_admin: true,
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

export function fullRepository(r: Repository): FullRepository {
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

// ---- The repositories every area serves ----

export const edge = repository(
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
export const web = repository("web", "The Graftyard web app.", "internal", {
  language: "TypeScript",
  stars: 38,
  topics: ["react", "tanstack"],
  pushed: "2026-11-07T17:40:00Z",
});
export const raft = repository(
  "raft",
  "Log, lease, transfer, and learners for each group.",
  "private",
  {
    language: "Rust",
    stars: 91,
    topics: ["raft", "consensus"],
    pushed: "2026-11-05T08:03:00Z",
  },
);

export const repositories = [edge, raft, web];

export function findRepo(owner: string, name: string): Repository | undefined {
  return repositories.find((r) => r.full_name.toLowerCase() === `${owner}/${name}`.toLowerCase());
}

// ---- The organization, its teams, and the branches every repository has ----

export const org: Schemas["organization-full"] = {
  login: "graftyard",
  id: users.org.id,
  node_id: users.org.node_id,
  url: `${API}/orgs/graftyard`,
  repos_url: `${API}/orgs/graftyard/repos`,
  events_url: `${API}/orgs/graftyard/events`,
  hooks_url: `${API}/orgs/graftyard/hooks`,
  issues_url: `${API}/orgs/graftyard/issues`,
  members_url: `${API}/orgs/graftyard/members{/member}`,
  public_members_url: `${API}/orgs/graftyard/public_members{/member}`,
  avatar_url: users.org.avatar_url,
  description: "A GHES-compatible forge on Cloudflare.",
  name: "Graftyard",
  company: null,
  blog: "https://graftyard.dev",
  location: "Everywhere Cloudflare is",
  email: null,
  is_verified: true,
  has_organization_projects: false,
  has_repository_projects: false,
  public_repos: 1,
  public_gists: 0,
  followers: 40,
  following: 0,
  html_url: `${HOST}/graftyard`,
  type: "Organization",
  total_private_repos: 2,
  owned_private_repos: 2,
  default_repository_permission: "read",
  default_repository_branch: "main",
  members_can_create_repositories: true,
  two_factor_requirement_enabled: true,
  members_can_create_public_repositories: false,
  members_can_create_private_repositories: true,
  members_can_create_internal_repositories: true,
  members_can_fork_private_repositories: false,
  web_commit_signoff_required: false,
  created_at: "2025-01-10T00:00:00Z",
  updated_at: "2026-10-01T00:00:00Z",
  archived_at: null,
};

function team(slug: string, name: string, description: string, permission: string) {
  const url = `${API}/orgs/graftyard/teams/${slug}`;
  const t: Schemas["team"] = {
    id: parseInt(sha(`team:${slug}`).slice(0, 6), 16),
    node_id: `T_${sha(`team:${slug}`).slice(0, 12)}`,
    name,
    slug,
    description,
    privacy: "closed",
    notification_setting: "notifications_enabled",
    permission,
    url,
    html_url: `${HOST}/orgs/graftyard/teams/${slug}`,
    members_url: `${url}/members{/member}`,
    repositories_url: `${url}/repos`,
    type: "organization",
    organization_id: org.id,
    parent: null,
  };
  return t;
}

export const teams = {
  maintainers: team("maintainers", "Maintainers", "Merge rights on every repository.", "maintain"),
  edge: team("edge", "Edge", "The Rust edge Worker and the wire crates.", "push"),
  storage: team("storage", "Storage", "Raft groups, folds, and the blob store.", "push"),
};

// Who belongs to which team. Org owners are mira and devi; everyone else is a member.
export const teamMembers: Record<keyof typeof teams, SimpleUser[]> = {
  maintainers: [users.mira, users.devi],
  edge: [users.mira, users.tomas, users.devi],
  storage: [users.lin, users.devi],
};
export const orgOwners = [users.mira, users.devi];
export const orgMembers = [users.mira, users.tomas, users.lin, users.devi];

// Branch names per repository. The core area's PR heads are among them, so every area
// that reads refs (code browser, branches, compare, Actions runs) agrees on the same set.
export const branches: Record<string, string[]> = {
  "graftyard/edge": [
    "main",
    "mira/idempotent-retry",
    "devi/ratelimit-on-errors",
    "lin/learner-reads",
    "tomas/label-objects",
    "release/0.4",
  ],
  "graftyard/raft": ["main", "lin/joint-consensus"],
  "graftyard/web": ["main", "lin/dark-diff"],
};

// The commit sha a branch points at. Matches core-db's PR head and base shas.
export const branchSha = (fullName: string, branch: string) => sha(`${fullName}:${branch}`);

// Tags per repository, oldest first. Releases are made from these.
export const tags: Record<string, string[]> = {
  "graftyard/edge": ["v0.1.0", "v0.2.0", "v0.3.0", "v0.4.0-rc.1"],
  "graftyard/raft": ["v0.1.0"],
  "graftyard/web": [],
};
