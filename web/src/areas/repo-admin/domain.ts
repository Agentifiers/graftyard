import type { Schemas } from "@/api/client";

// The tables repository administration is built from. The mock and the pages both read
// them, so a role, a merge-message preset, a webhook event, or a ruleset rule is named
// once.

// ---- Repository roles ----

export const ROLES = ["read", "triage", "write", "maintain", "admin"] as const;
export type Role = (typeof ROLES)[number];

// GHES spells three of the five roles differently in different places: collaborators and
// invitations say read and write, teams and the add-collaborator body say pull and push.
type ApiPermission = "pull" | "triage" | "push" | "maintain" | "admin";

export const ROLE_INFO: Record<Role, { api: ApiPermission; description: string }> = {
  read: {
    api: "pull",
    description: "Read and clone, open and comment on issues and pull requests.",
  },
  triage: {
    api: "triage",
    description: "Read, and manage issues and pull requests without write access.",
  },
  write: { api: "push", description: "Triage, and push to the repository." },
  maintain: {
    api: "maintain",
    description: "Write, and manage the repository without sensitive actions.",
  },
  admin: { api: "admin", description: "Full access, including settings, security, and deletion." },
};

export function roleOf(permission: string): Role | undefined {
  if (permission === "pull") return "read";
  if (permission === "push") return "write";
  return ROLES.find((r) => r === permission);
}

// The permissions object GHES returns: every role at or below the one held is true.
export function permissionsFor(role: Role) {
  const at = ROLES.indexOf(role);
  return {
    pull: at >= 0,
    triage: at >= 1,
    push: at >= 2,
    maintain: at >= 3,
    admin: at >= 4,
  };
}

// ---- Merge commit message presets ----
// The UI offers named presets; the API stores a title and a message source for each.

type SquashTitle = NonNullable<Schemas["repository"]["squash_merge_commit_title"]>;
type SquashMessage = NonNullable<Schemas["repository"]["squash_merge_commit_message"]>;
type MergeTitle = NonNullable<Schemas["repository"]["merge_commit_title"]>;
type MergeMessage = NonNullable<Schemas["repository"]["merge_commit_message"]>;

export const SQUASH_PRESETS: {
  id: string;
  label: string;
  title: SquashTitle;
  message: SquashMessage;
}[] = [
  {
    id: "default",
    label: "Default message",
    title: "COMMIT_OR_PR_TITLE",
    message: "COMMIT_MESSAGES",
  },
  { id: "pr-title", label: "Pull request title", title: "PR_TITLE", message: "BLANK" },
  {
    id: "pr-title-commits",
    label: "Pull request title and commit details",
    title: "PR_TITLE",
    message: "COMMIT_MESSAGES",
  },
  {
    id: "pr-title-body",
    label: "Pull request title and description",
    title: "PR_TITLE",
    message: "PR_BODY",
  },
];

export const MERGE_PRESETS: {
  id: string;
  label: string;
  title: MergeTitle;
  message: MergeMessage;
}[] = [
  { id: "default", label: "Default message", title: "MERGE_MESSAGE", message: "PR_TITLE" },
  { id: "pr-title", label: "Pull request title", title: "PR_TITLE", message: "BLANK" },
  {
    id: "pr-title-body",
    label: "Pull request title and description",
    title: "PR_TITLE",
    message: "PR_BODY",
  },
];

export function presetOf<P extends { id: string; title: string; message: string }>(
  presets: P[],
  title: string | undefined,
  message: string | undefined,
): P {
  return presets.find((p) => p.title === title && p.message === message) ?? presets[0];
}

// ---- Webhook events ----

export const HOOK_EVENTS: { name: string; description: string }[] = [
  { name: "push", description: "Git push to a repository." },
  {
    name: "pull_request",
    description: "Pull request opened, closed, reopened, edited, or synchronized.",
  },
  {
    name: "pull_request_review",
    description: "Pull request review submitted, edited, or dismissed.",
  },
  { name: "issues", description: "Issue opened, edited, closed, reopened, labeled, or assigned." },
  {
    name: "issue_comment",
    description: "Issue or pull request comment created, edited, or deleted.",
  },
  { name: "create", description: "Branch or tag created." },
  { name: "delete", description: "Branch or tag deleted." },
  { name: "release", description: "Release created, edited, published, or deleted." },
  { name: "check_run", description: "Check run created, requested, rerequested, or completed." },
  { name: "check_suite", description: "Check suite requested, rerequested, or completed." },
  { name: "workflow_run", description: "Workflow run requested, in progress, or completed." },
  { name: "deployment", description: "Repository deployment created." },
  { name: "deployment_status", description: "Deployment status updated from the API." },
  { name: "fork", description: "Repository forked." },
  { name: "member", description: "Collaborator added, removed, or changed permission." },
  {
    name: "repository",
    description: "Repository created, renamed, transferred, archived, or deleted.",
  },
];

// ---- Ruleset rules ----

export type Rule = Schemas["repository-rule"];
export type RuleType = Rule["type"];
export type RulesetTarget = "branch" | "tag";

// The rules the builder offers, in the order GHES lists them. `make` is the rule with its
// default parameters, used when the rule is switched on.
export const RULE_KINDS: {
  type: RuleType;
  label: string;
  description: string;
  targets: RulesetTarget[];
  make: () => Rule;
}[] = [
  {
    type: "creation",
    label: "Restrict creations",
    description: "Only allow users with bypass permission to create matching refs.",
    targets: ["branch", "tag"],
    make: () => ({ type: "creation" }),
  },
  {
    type: "update",
    label: "Restrict updates",
    description: "Only allow users with bypass permission to update matching refs.",
    targets: ["branch", "tag"],
    make: () => ({ type: "update", parameters: { update_allows_fetch_and_merge: false } }),
  },
  {
    type: "deletion",
    label: "Restrict deletions",
    description: "Only allow users with bypass permissions to delete matching refs.",
    targets: ["branch", "tag"],
    make: () => ({ type: "deletion" }),
  },
  {
    type: "required_linear_history",
    label: "Require linear history",
    description: "Prevent merge commits from being pushed to matching refs.",
    targets: ["branch"],
    make: () => ({ type: "required_linear_history" }),
  },
  {
    type: "required_signatures",
    label: "Require signed commits",
    description: "Commits pushed to matching refs must have verified signatures.",
    targets: ["branch", "tag"],
    make: () => ({ type: "required_signatures" }),
  },
  {
    type: "pull_request",
    label: "Require a pull request before merging",
    description:
      "All commits must be made to a non-target branch and submitted through a pull request.",
    targets: ["branch"],
    make: () => ({
      type: "pull_request",
      parameters: {
        allowed_merge_methods: ["merge", "squash", "rebase"],
        dismiss_stale_reviews_on_push: false,
        require_code_owner_review: false,
        require_last_push_approval: false,
        required_approving_review_count: 1,
        required_review_thread_resolution: false,
      },
    }),
  },
  {
    type: "required_status_checks",
    label: "Require status checks to pass",
    description: "Choose which status checks must pass before the ref is updated.",
    targets: ["branch"],
    make: () => ({
      type: "required_status_checks",
      parameters: { required_status_checks: [], strict_required_status_checks_policy: false },
    }),
  },
  {
    type: "non_fast_forward",
    label: "Block force pushes",
    description: "Prevent users with push access from force pushing to refs.",
    targets: ["branch", "tag"],
    make: () => ({ type: "non_fast_forward" }),
  },
  {
    type: "commit_message_pattern",
    label: "Commit message pattern",
    description: "Commit messages pushed to matching refs must match a pattern.",
    targets: ["branch", "tag"],
    make: () => ({
      type: "commit_message_pattern",
      parameters: { operator: "starts_with", pattern: "", negate: false },
    }),
  },
  {
    type: "branch_name_pattern",
    label: "Branch name pattern",
    description: "Branch names must match a pattern.",
    targets: ["branch"],
    make: () => ({
      type: "branch_name_pattern",
      parameters: { operator: "regex", pattern: "", negate: false },
    }),
  },
  {
    type: "tag_name_pattern",
    label: "Tag name pattern",
    description: "Tag names must match a pattern.",
    targets: ["tag"],
    make: () => ({
      type: "tag_name_pattern",
      parameters: { operator: "starts_with", pattern: "v", negate: false },
    }),
  },
];

export const ruleLabel = (type: string) => RULE_KINDS.find((k) => k.type === type)?.label ?? type;

// Repository roles a ruleset may exempt. The ids are GHES's built-in role ids.
export const REPOSITORY_ROLE_IDS: { id: number; label: string }[] = [
  { id: 5, label: "Repository admin" },
  { id: 2, label: "Maintain" },
  { id: 4, label: "Write" },
];

// GitHub's fnmatch-style branch patterns, as branch protection and ruleset conditions
// use them: * matches within a path segment, ** across segments, ? one character.
export function matchesPattern(pattern: string, name: string): boolean {
  const source = pattern
    .replace(/[.+^${}()|\\]/g, "\\$&")
    .replace(/\*\*/g, "\u0000")
    .replace(/\*/g, "[^/]*")
    .replace(/\u0000/g, ".*")
    .replace(/\?/g, ".");
  return new RegExp(`^${source}$`).test(name);
}

// Ruleset ref conditions: refs/heads/<pattern>, ~DEFAULT_BRANCH, or ~ALL.
export function refMatches(
  condition: string,
  ref: string,
  defaultBranch: string,
  target: RulesetTarget,
): boolean {
  if (condition === "~ALL") return true;
  const prefix = target === "tag" ? "refs/tags/" : "refs/heads/";
  if (condition === "~DEFAULT_BRANCH") return target === "branch" && ref === defaultBranch;
  const pattern = condition.startsWith(prefix) ? condition.slice(prefix.length) : condition;
  return matchesPattern(pattern, ref);
}

// Repository names GHES accepts: letters, digits, dots, hyphens, and underscores.
// Anything else becomes a hyphen, as GHES does when it creates the repository.
export function normalizeRepoName(name: string): string {
  return name.trim().replace(/[^A-Za-z0-9._-]+/g, "-");
}

export const TOPIC_RE = /^[a-z0-9][a-z0-9-]{0,49}$/;
