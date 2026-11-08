import type { Access, AppPermissions, PermissionKey } from "./ui-api";

// The permissions a fine-grained token or a GitHub App can ask for, and the levels each
// one allows. The levels come from GHES's `app-permissions` schema (workflows is write
// only, profile is write only), so the matrix never offers a level the wire rejects.

export type PermissionGroup = "repository" | "organization" | "account";

export type Permission = {
  key: PermissionKey;
  label: string;
  group: PermissionGroup;
  levels: readonly Access[];
  hint: string;
};

const RW = ["read", "write"] as const;

export const PERMISSIONS: readonly Permission[] = [
  {
    key: "actions",
    label: "Actions",
    group: "repository",
    levels: RW,
    hint: "Workflows, workflow runs, and artifacts.",
  },
  {
    key: "administration",
    label: "Administration",
    group: "repository",
    levels: RW,
    hint: "Repository creation, deletion, settings, teams, and collaborators.",
  },
  {
    key: "checks",
    label: "Checks",
    group: "repository",
    levels: RW,
    hint: "Check runs and check suites.",
  },
  {
    key: "contents",
    label: "Contents",
    group: "repository",
    levels: RW,
    hint: "Repository contents, commits, branches, downloads, releases, and merges.",
  },
  {
    key: "deployments",
    label: "Deployments",
    group: "repository",
    levels: RW,
    hint: "Deployments and deployment statuses.",
  },
  {
    key: "environments",
    label: "Environments",
    group: "repository",
    levels: RW,
    hint: "Environments, their secrets and variables.",
  },
  {
    key: "issues",
    label: "Issues",
    group: "repository",
    levels: RW,
    hint: "Issues and related comments, assignees, labels, and milestones.",
  },
  {
    key: "metadata",
    label: "Metadata",
    group: "repository",
    levels: ["read"],
    hint: "Search repositories, list collaborators, and access repository metadata.",
  },
  {
    key: "pages",
    label: "Pages",
    group: "repository",
    levels: RW,
    hint: "Pages statuses, configuration, and builds.",
  },
  {
    key: "pull_requests",
    label: "Pull requests",
    group: "repository",
    levels: RW,
    hint: "Pull requests and related comments, assignees, labels, milestones, and merges.",
  },
  {
    key: "repository_hooks",
    label: "Webhooks",
    group: "repository",
    levels: RW,
    hint: "Repository webhooks and their deliveries.",
  },
  { key: "secrets", label: "Secrets", group: "repository", levels: RW, hint: "Actions secrets." },
  {
    key: "security_events",
    label: "Code scanning alerts",
    group: "repository",
    levels: RW,
    hint: "Code scanning alerts and analyses.",
  },
  {
    key: "statuses",
    label: "Commit statuses",
    group: "repository",
    levels: RW,
    hint: "Commit statuses.",
  },
  {
    key: "workflows",
    label: "Workflows",
    group: "repository",
    levels: ["write"],
    hint: "Update GitHub Actions workflow files.",
  },
  {
    key: "members",
    label: "Members",
    group: "organization",
    levels: RW,
    hint: "Organization members and teams.",
  },
  {
    key: "organization_administration",
    label: "Administration",
    group: "organization",
    levels: RW,
    hint: "Organization settings.",
  },
  {
    key: "organization_hooks",
    label: "Webhooks",
    group: "organization",
    levels: RW,
    hint: "Organization webhooks.",
  },
  {
    key: "organization_secrets",
    label: "Secrets",
    group: "organization",
    levels: RW,
    hint: "Organization Actions secrets.",
  },
  {
    key: "organization_self_hosted_runners",
    label: "Self-hosted runners",
    group: "organization",
    levels: RW,
    hint: "Self-hosted runners and runner groups.",
  },
  {
    key: "email_addresses",
    label: "Email addresses",
    group: "account",
    levels: RW,
    hint: "Your email addresses.",
  },
  {
    key: "followers",
    label: "Followers",
    group: "account",
    levels: RW,
    hint: "Who follows you, and whom you follow.",
  },
  {
    key: "git_ssh_keys",
    label: "Git SSH keys",
    group: "account",
    levels: RW,
    hint: "Your SSH authentication keys.",
  },
  { key: "gpg_keys", label: "GPG keys", group: "account", levels: RW, hint: "Your GPG keys." },
  {
    key: "profile",
    label: "Profile",
    group: "account",
    levels: ["write"],
    hint: "Your profile settings.",
  },
  {
    key: "starring",
    label: "Starring",
    group: "account",
    levels: RW,
    hint: "Starred repositories.",
  },
];

export const GROUP_LABELS: Record<PermissionGroup, string> = {
  repository: "Repository permissions",
  organization: "Organization permissions",
  account: "Account permissions",
};

// Sets one permission. Any repository permission needs metadata read, as on GitHub, so
// metadata follows the others and cannot be dropped while one is set.
export function setPermission(
  current: AppPermissions,
  key: PermissionKey,
  level: Access | "none",
): AppPermissions {
  const next: Record<string, Access> = { ...current } as Record<string, Access>;
  if (level === "none") delete next[key];
  else next[key] = level;
  const repoKeys = PERMISSIONS.filter((p) => p.group === "repository" && p.key !== "metadata");
  if (repoKeys.some((p) => next[p.key])) next.metadata = "read";
  return next as AppPermissions;
}

export function metadataLocked(current: AppPermissions) {
  return PERMISSIONS.some(
    (p) => p.group === "repository" && p.key !== "metadata" && current[p.key],
  );
}

export function permissionSummary(permissions: AppPermissions): string {
  const set = PERMISSIONS.filter((p) => permissions[p.key]);
  if (set.length === 0) return "No permissions";
  return set.map((p) => `${p.label}: ${permissions[p.key]}`).join(", ");
}

// Classic token scopes, as GHES 3.22 lists them. A parent scope implies its children.
export type Scope = { name: string; description: string; children?: Scope[] };

export const SCOPES: readonly Scope[] = [
  {
    name: "repo",
    description: "Full control of private repositories",
    children: [
      { name: "repo:status", description: "Access commit status" },
      { name: "repo_deployment", description: "Access deployment status" },
      { name: "public_repo", description: "Access public repositories" },
      { name: "repo:invite", description: "Access repository invitations" },
      { name: "security_events", description: "Read and write security events" },
    ],
  },
  { name: "workflow", description: "Update GitHub Action workflows" },
  {
    name: "write:packages",
    description: "Upload packages to GitHub Package Registry",
    children: [
      { name: "read:packages", description: "Download packages from GitHub Package Registry" },
    ],
  },
  { name: "delete:packages", description: "Delete packages from GitHub Package Registry" },
  {
    name: "admin:org",
    description: "Full control of orgs and teams, read and write org projects",
    children: [
      {
        name: "write:org",
        description: "Read and write org and team membership, read and write org projects",
      },
      { name: "read:org", description: "Read org and team membership, read org projects" },
      { name: "manage_runners:org", description: "Manage org runners and runner groups" },
    ],
  },
  {
    name: "admin:public_key",
    description: "Full control of user public keys",
    children: [
      { name: "write:public_key", description: "Write user public keys" },
      { name: "read:public_key", description: "Read user public keys" },
    ],
  },
  {
    name: "admin:repo_hook",
    description: "Full control of repository hooks",
    children: [
      { name: "write:repo_hook", description: "Write repository hooks" },
      { name: "read:repo_hook", description: "Read repository hooks" },
    ],
  },
  { name: "admin:org_hook", description: "Full control of organization hooks" },
  { name: "gist", description: "Create gists" },
  { name: "notifications", description: "Access notifications" },
  {
    name: "user",
    description: "Update ALL user data",
    children: [
      { name: "read:user", description: "Read ALL user profile data" },
      { name: "user:email", description: "Access user email addresses (read-only)" },
      { name: "user:follow", description: "Follow and unfollow users" },
    ],
  },
  { name: "delete_repo", description: "Delete repositories" },
  {
    name: "write:discussion",
    description: "Read and write team discussions",
    children: [{ name: "read:discussion", description: "Read team discussions" }],
  },
  {
    name: "admin:enterprise",
    description: "Full control of enterprises",
    children: [
      {
        name: "manage_runners:enterprise",
        description: "Manage enterprise runners and runner groups",
      },
      { name: "read:enterprise", description: "Read enterprise profile data" },
    ],
  },
  {
    name: "admin:gpg_key",
    description: "Full control of public user GPG keys",
    children: [
      { name: "write:gpg_key", description: "Write public user GPG keys" },
      { name: "read:gpg_key", description: "Read public user GPG keys" },
    ],
  },
  {
    name: "admin:ssh_signing_key",
    description: "Full control of public user SSH signing keys",
    children: [
      { name: "write:ssh_signing_key", description: "Write public user SSH signing keys" },
      { name: "read:ssh_signing_key", description: "Read public user SSH signing keys" },
    ],
  },
  { name: "site_admin", description: "Access the site administration API" },
];

export const SCOPE_NAMES = new Set(
  SCOPES.flatMap((s) => [s.name, ...(s.children ?? []).map((c) => c.name)]),
);

// Plain words for the consent and device pages.
export const SCOPE_WORDS: Record<string, string> = {
  repo: "Read and write all your repositories, including private ones",
  public_repo: "Read and write your public repositories",
  "read:org": "Read your organization and team membership",
  "admin:org": "Manage your organizations and teams",
  "admin:public_key": "Add and remove your SSH keys",
  "read:user": "Read your profile",
  "user:email": "Read your email addresses",
  user: "Read and update your profile and email addresses",
  gist: "Create gists",
  workflow: "Change GitHub Actions workflow files",
  notifications: "Read and mark your notifications",
  "write:packages": "Publish packages",
  "read:packages": "Download packages",
  delete_repo: "Delete repositories",
  site_admin: "Use the site administration API",
};

export const scopeWords = (s: string) => SCOPE_WORDS[s] ?? `The ${s} scope`;

// The webhook events a GitHub App can subscribe to, with the permission each needs.
export const APP_EVENTS: readonly { name: string; needs: PermissionKey }[] = [
  { name: "check_run", needs: "checks" },
  { name: "check_suite", needs: "checks" },
  { name: "create", needs: "contents" },
  { name: "delete", needs: "contents" },
  { name: "deployment", needs: "deployments" },
  { name: "deployment_status", needs: "deployments" },
  { name: "issue_comment", needs: "issues" },
  { name: "issues", needs: "issues" },
  { name: "label", needs: "issues" },
  { name: "member", needs: "metadata" },
  { name: "milestone", needs: "issues" },
  { name: "public", needs: "metadata" },
  { name: "pull_request", needs: "pull_requests" },
  { name: "pull_request_review", needs: "pull_requests" },
  { name: "pull_request_review_comment", needs: "pull_requests" },
  { name: "push", needs: "contents" },
  { name: "release", needs: "contents" },
  { name: "repository", needs: "metadata" },
  { name: "status", needs: "statuses" },
  { name: "workflow_job", needs: "actions" },
  { name: "workflow_run", needs: "actions" },
];
