import type { Schemas } from "@/api/client";
import { matchesPattern, permissionsFor, type Role } from "@/areas/repo-admin/domain";
import type { ProtectionRule } from "@/areas/repo-admin/private-api";
import {
  actionsApp,
  API,
  branches,
  HOST,
  id,
  org,
  repository,
  sha,
  teams,
  users,
  type Repository,
  type SimpleUser,
} from "../world";
import { db } from "./core-db";

// Repository administration's records: collaborators, invitations, team grants, branch
// protection, rulesets, autolinks, webhooks and their deliveries, deploy keys, pre-receive
// hooks, and installed apps. Each repository's record is keyed by its id, so a rename or a
// transfer through db.moveRepo keeps every setting.

type Collaborator = Schemas["collaborator"];
type Invitation = Schemas["repository-invitation"];
type Team = Schemas["team"];
type Ruleset = Schemas["repository-ruleset"];
type RuleSuite = Schemas["rule-suites"][number] & { evaluations: RuleEvaluation[] };
type RuleEvaluation = NonNullable<Schemas["rule-suite"]["rule_evaluations"]>[number];
type Autolink = Schemas["autolink"];
type Hook = Schemas["hook"];
type Delivery = Schemas["hook-delivery"];
type DeployKey = Schemas["deploy-key"];
type PreReceiveHook = Schemas["repository-pre-receive-hook"];
type Installation = Schemas["installation"];
type Protection = Schemas["branch-protection"];

type Grant = { user: SimpleUser; role: Role };

export type AdminState = {
  collaborators: Grant[];
  invitations: Invitation[];
  teams: { slug: keyof typeof teams; role: Role }[];
  protections: Map<string, { protection: Protection; created_at: string }>;
  rulesets: Ruleset[];
  ruleHistory: Map<number, Schemas["ruleset-version"][]>;
  suites: RuleSuite[];
  autolinks: Autolink[];
  hooks: Hook[];
  deliveries: Map<number, Delivery[]>;
  keys: DeployKey[];
  preReceive: (PreReceiveHook & { default_enforcement: string; configurable: boolean })[];
  lfs: "enabled" | "enabling" | "disabled";
  lfsBytes: number;
  // Branch names. Seeded from world.ts by the repository's first name, so a rename keeps them.
  branches: string[];
  // Set on a fork or a repository generated from a template.
  parent?: Repository;
  template?: Repository;
  // Forks that existed before this session. The world gives each repository forks_count 3
  // but no fork repositories, so these stand behind that count; they are not in core-db, so
  // they stay out of other areas' lists. Forks made this session are real repositories.
  seededForks: Repository[];
};

// Every login the instance knows, for invitations, restrictions, and autocomplete.
export const people: SimpleUser[] = [users.mira, users.tomas, users.lin, users.devi];

export const findPerson = (login: string) =>
  people.find((p) => p.login.toLowerCase() === login.toLowerCase());

// ---- Wire shapes ----

export function collaborator(g: Grant): Collaborator {
  return {
    ...g.user,
    permissions: permissionsFor(g.role),
    role_name: g.role,
  };
}

export function minimal(r: Repository): Schemas["minimal-repository"] {
  const l = r.license;
  return {
    ...r,
    license: l && {
      key: l.key,
      name: l.name,
      url: l.url,
      node_id: l.node_id,
      ...(l.spdx_id && { spdx_id: l.spdx_id }),
    },
  };
}

export function teamFor(slug: keyof typeof teams, role: Role): Team {
  const t = teams[slug];
  return {
    ...t,
    permission: role === "read" ? "pull" : role === "write" ? "push" : role,
    permissions: permissionsFor(role),
  };
}

export function invitation(
  repo: Repository,
  invitee: SimpleUser,
  role: Role,
  at: string,
): Invitation {
  const n = id();
  return {
    id: n,
    node_id: `RI_${sha(`${n}`).slice(0, 12)}`,
    repository: minimal(repo),
    invitee,
    inviter: users.mira,
    permissions: role,
    created_at: at,
    expired: false,
    url: `${repo.url}/invitations/${n}`,
    html_url: `${repo.html_url}/invitations`,
  };
}

const NO_RESTRICTIONS = undefined;

// A protection record in the shape GET .../branches/{branch}/protection answers.
export function protectionRecord(
  repo: Repository,
  branch: string,
  p: {
    reviews: {
      count: number;
      dismissStale: boolean;
      codeOwners: boolean;
      lastPush: boolean;
    } | null;
    checks: { strict: boolean; contexts: string[] } | null;
    enforceAdmins: boolean;
    signatures: boolean;
    linear: boolean;
    forcePushes: boolean;
    deletions: boolean;
    conversations: boolean;
    restrict: { users: string[]; teams: string[] } | null;
    lock?: boolean;
  },
): Protection {
  const url = `${repo.url}/branches/${encodeURIComponent(branch)}/protection`;
  return {
    url,
    enabled: true,
    name: branch,
    protection_url: url,
    required_status_checks: p.checks
      ? {
          url: `${url}/required_status_checks`,
          enforcement_level: "non_admins",
          strict: p.checks.strict,
          contexts: p.checks.contexts,
          checks: p.checks.contexts.map((context) => ({
            context,
            app_id: context.startsWith("ci/") ? null : (actionsApp?.id ?? null),
          })),
          contexts_url: `${url}/required_status_checks/contexts`,
        }
      : undefined,
    enforce_admins: { url: `${url}/enforce_admins`, enabled: p.enforceAdmins },
    required_pull_request_reviews: p.reviews
      ? {
          url: `${url}/required_pull_request_reviews`,
          dismiss_stale_reviews: p.reviews.dismissStale,
          require_code_owner_reviews: p.reviews.codeOwners,
          required_approving_review_count: p.reviews.count,
          require_last_push_approval: p.reviews.lastPush,
        }
      : undefined,
    restrictions: p.restrict
      ? {
          url: `${url}/restrictions`,
          users_url: `${url}/restrictions/users`,
          teams_url: `${url}/restrictions/teams`,
          apps_url: `${url}/restrictions/apps`,
          users: p.restrict.users.flatMap((l) => {
            const u = findPerson(l);
            return u ? [{ ...u, gravatar_id: u.gravatar_id ?? "" }] : [];
          }),
          teams: p.restrict.teams.flatMap((s) =>
            s in teams ? [teamFor(s as keyof typeof teams, "write")] : [],
          ),
          apps: [],
        }
      : NO_RESTRICTIONS,
    required_signatures: { url: `${url}/required_signatures`, enabled: p.signatures },
    required_linear_history: { enabled: p.linear },
    allow_force_pushes: { enabled: p.forcePushes },
    allow_deletions: { enabled: p.deletions },
    block_creations: { enabled: false },
    required_conversation_resolution: { enabled: p.conversations },
    lock_branch: { enabled: p.lock ?? false },
    allow_fork_syncing: { enabled: false },
  };
}

// GET .../protection answers protected-branch, whose nested fields are all required.
export function protectedBranch(p: Protection): Schemas["protected-branch"] {
  const url = p.url ?? "";
  return {
    url,
    required_status_checks: p.required_status_checks && {
      url: p.required_status_checks.url ?? `${url}/required_status_checks`,
      strict: p.required_status_checks.strict ?? false,
      contexts: p.required_status_checks.contexts,
      checks: p.required_status_checks.checks,
      contexts_url:
        p.required_status_checks.contexts_url ?? `${url}/required_status_checks/contexts`,
    },
    required_pull_request_reviews: p.required_pull_request_reviews && {
      url: p.required_pull_request_reviews.url ?? `${url}/required_pull_request_reviews`,
      dismiss_stale_reviews: p.required_pull_request_reviews.dismiss_stale_reviews,
      require_code_owner_reviews: p.required_pull_request_reviews.require_code_owner_reviews,
      required_approving_review_count:
        p.required_pull_request_reviews.required_approving_review_count,
      require_last_push_approval: p.required_pull_request_reviews.require_last_push_approval,
    },
    required_signatures: p.required_signatures,
    enforce_admins: p.enforce_admins,
    required_linear_history: { enabled: p.required_linear_history?.enabled ?? false },
    allow_force_pushes: { enabled: p.allow_force_pushes?.enabled ?? false },
    allow_deletions: { enabled: p.allow_deletions?.enabled ?? false },
    restrictions: p.restrictions,
    required_conversation_resolution: p.required_conversation_resolution,
    block_creations: { enabled: p.block_creations?.enabled ?? false },
    lock_branch: { enabled: p.lock_branch?.enabled ?? false },
    allow_fork_syncing: { enabled: p.allow_fork_syncing?.enabled ?? false },
  };
}

function hook(
  repo: Repository,
  seed: { url: string; events: string[]; active: boolean; at: string },
): Hook {
  const n = id();
  const base = `${repo.url}/hooks/${n}`;
  return {
    type: "Repository",
    id: n,
    name: "web",
    active: seed.active,
    events: seed.events,
    config: { url: seed.url, content_type: "json", insecure_ssl: "0", secret: "********" },
    updated_at: seed.at,
    created_at: seed.at,
    url: base,
    test_url: `${base}/test`,
    ping_url: `${base}/pings`,
    deliveries_url: `${base}/deliveries`,
    last_response: { code: null, status: "unused", message: null },
  };
}

// A delivery with the request GHES sent and the response the receiver gave.
export function delivery(
  repo: Repository,
  h: Hook,
  event: string,
  action: string | null,
  at: string,
  status: number,
  redelivery = false,
): Delivery {
  const n = id();
  const guid = `${sha(`delivery:${n}`).slice(0, 8)}-${sha(`g:${n}`).slice(0, 4)}-11ef-${sha(`h:${n}`).slice(0, 4)}-${sha(`i:${n}`).slice(0, 12)}`;
  const ok = status >= 200 && status < 300;
  return {
    id: n,
    guid,
    delivered_at: at,
    redelivery,
    duration: Math.round((0.08 + (n % 7) * 0.05) * 100) / 100,
    status: ok ? "OK" : status === 0 ? "Timed out" : "Internal Server Error",
    status_code: status,
    event,
    action,
    installation_id: null,
    repository_id: repo.id,
    url: h.config.url,
    request: {
      headers: {
        Accept: "*/*",
        "Content-Type":
          h.config.content_type === "form"
            ? "application/x-www-form-urlencoded"
            : "application/json",
        "User-Agent": "GitHub-Hookshot/graftyard",
        "X-GitHub-Delivery": guid,
        "X-GitHub-Enterprise-Host": "ghes.graftyard.test",
        "X-GitHub-Enterprise-Version": "3.22.0",
        "X-GitHub-Event": event,
        "X-GitHub-Hook-ID": String(h.id),
        "X-GitHub-Hook-Installation-Target-ID": String(repo.id),
        "X-GitHub-Hook-Installation-Target-Type": "repository",
        "X-Hub-Signature-256": `sha256=${sha(`sig:${n}`)}${sha(`sig2:${n}`).slice(0, 24)}`,
      },
      payload: payloadFor(repo, h, event, action),
    },
    response: {
      headers: ok
        ? {
            "Content-Type": "text/plain; charset=utf-8",
            "Content-Length": "2",
            Date: new Date(at).toUTCString(),
          }
        : status === 0
          ? null
          : { "Content-Type": "text/html", Date: new Date(at).toUTCString() },
      payload: ok ? "ok" : status === 0 ? null : "<h1>500 Internal Server Error</h1>",
    },
  };
}

function payloadFor(repo: Repository, h: Hook, event: string, action: string | null) {
  const sender = { login: users.mira.login, id: users.mira.id, type: "User" };
  const repository = { id: repo.id, full_name: repo.full_name, private: repo.private };
  if (event === "ping") {
    return {
      zen: "Keep it logically awesome.",
      hook_id: h.id,
      hook: {
        type: h.type,
        id: h.id,
        active: h.active,
        events: h.events,
        config: { ...h.config, secret: undefined },
      },
      repository,
      sender,
    };
  }
  if (event === "push") {
    const after = sha(`${repo.full_name}:main`);
    return {
      ref: `refs/heads/${repo.default_branch}`,
      before: sha(`${repo.full_name}:main:before`),
      after,
      commits: [
        {
          id: after,
          message: "Merge pull request #12 from graftyard/mira/idempotent-retry",
          author: { name: "Mira Okafor", username: "mira" },
        },
      ],
      pusher: { name: "mira", email: "mira@graftyard.test" },
      repository,
      sender,
    };
  }
  return { action, number: 12, repository, sender };
}

const keyText = (seed: string, kind = "ssh-ed25519") =>
  `${kind} AAAAC3NzaC1lZDI1NTE5AAAAI${btoa(sha(seed)).slice(0, 43)}`;

export function deployKey(
  repo: Repository,
  title: string,
  key: string,
  readOnly: boolean,
  at: string,
  lastUsed: string | null,
): DeployKey {
  const n = id();
  return {
    id: n,
    key,
    url: `${repo.url}/keys/${n}`,
    title,
    verified: true,
    created_at: at,
    read_only: readOnly,
    added_by: "mira",
    last_used: lastUsed,
    enabled: true,
  };
}

function installation(
  slug: string,
  appId: number,
  permissions: Installation["permissions"],
  events: string[],
  at: string,
  selection: "all" | "selected",
): Installation {
  const n = 40_000 + appId;
  return {
    id: n,
    account: users.org,
    repository_selection: selection,
    access_tokens_url: `${API}/app/installations/${n}/access_tokens`,
    repositories_url: `${API}/installation/repositories`,
    html_url: `${HOST}/organizations/graftyard/settings/installations/${n}`,
    app_id: appId,
    client_id: `Iv1.${sha(slug).slice(0, 16)}`,
    target_id: org.id,
    target_type: "Organization",
    permissions,
    events,
    created_at: at,
    updated_at: at,
    single_file_name: null,
    app_slug: slug,
    suspended_by: null,
    suspended_at: null,
  };
}

// Apps installed on the organization, and which repositories each reaches.
export const installations: { install: Installation; repos: "all" | string[] }[] = [
  {
    install: installation(
      "github-actions",
      actionsApp?.id ?? 15368,
      {
        actions: "write",
        checks: "write",
        contents: "write",
        metadata: "read",
        pull_requests: "write",
      },
      ["check_run", "pull_request", "push", "workflow_run"],
      "2025-01-10T00:00:00Z",
      "all",
    ),
    repos: "all",
  },
  {
    install: installation(
      "graftyard-agents",
      9101,
      {
        contents: "write",
        issues: "write",
        metadata: "read",
        pull_requests: "write",
        checks: "read",
      },
      ["issues", "issue_comment", "pull_request", "pull_request_review"],
      "2026-06-02T12:00:00Z",
      "selected",
    ),
    repos: ["graftyard/edge", "graftyard/web"],
  },
  {
    install: installation(
      "release-drafter",
      9233,
      { contents: "write", metadata: "read", pull_requests: "read" },
      ["pull_request", "push"],
      "2026-02-14T08:30:00Z",
      "selected",
    ),
    repos: ["graftyard/edge"],
  },
];

// ---- Seed ----

const states = new Map<number, AdminState>();

function empty(names: string[] = []): AdminState {
  return {
    branches: names,
    collaborators: [],
    invitations: [],
    teams: [],
    protections: new Map(),
    rulesets: [],
    ruleHistory: new Map(),
    suites: [],
    autolinks: [],
    hooks: [],
    deliveries: new Map(),
    keys: [],
    preReceive: preReceiveDefaults(),
    lfs: "disabled",
    lfsBytes: 0,
    seededForks: [],
  };
}

function preReceiveDefaults(): AdminState["preReceive"] {
  return [
    {
      id: 1,
      name: "reject-large-files",
      enforcement: "enabled",
      default_enforcement: "enabled",
      configurable: false,
      configuration_url: `${API}/admin/pre-receive-hooks/1`,
    },
    {
      id: 2,
      name: "require-signed-tags",
      enforcement: "testing",
      default_enforcement: "testing",
      configurable: true,
      configuration_url: `${API}/admin/pre-receive-hooks/2`,
    },
    {
      id: 3,
      name: "block-secrets",
      enforcement: "disabled",
      default_enforcement: "disabled",
      configurable: true,
      configuration_url: `${API}/admin/pre-receive-hooks/3`,
    },
  ];
}

function ruleset(
  repo: Repository,
  seed: Omit<Ruleset, "id" | "source" | "source_type" | "node_id" | "_links">,
): Ruleset {
  const n = 7000 + id();
  return {
    id: n,
    source: repo.full_name,
    source_type: "Repository",
    node_id: `RRS_${sha(`${n}`).slice(0, 12)}`,
    _links: {
      self: { href: `${repo.url}/rulesets/${n}` },
      html: { href: `${repo.html_url}/rules/${n}` },
    },
    current_user_can_bypass: "never",
    ...seed,
  };
}

function seedEdge(repo: Repository): AdminState {
  const s = empty(branches[repo.full_name]);
  s.collaborators = [
    { user: users.tomas, role: "write" },
    { user: users.lin, role: "triage" },
  ];
  s.teams = [
    { slug: "maintainers", role: "maintain" },
    { slug: "edge", role: "write" },
  ];
  s.protections.set("main", {
    created_at: "2025-03-04T09:00:00Z",
    protection: protectionRecord(repo, "main", {
      reviews: { count: 2, dismissStale: true, codeOwners: true, lastPush: false },
      checks: { strict: true, contexts: ["test (ubuntu-latest)", "clippy", "rustfmt"] },
      enforceAdmins: false,
      signatures: false,
      linear: true,
      forcePushes: false,
      deletions: false,
      conversations: true,
      restrict: null,
    }),
  });
  s.protections.set("release/*", {
    created_at: "2026-05-11T14:00:00Z",
    protection: protectionRecord(repo, "release/*", {
      reviews: { count: 1, dismissStale: false, codeOwners: false, lastPush: true },
      checks: { strict: false, contexts: ["test (ubuntu-latest)"] },
      enforceAdmins: true,
      signatures: true,
      linear: false,
      forcePushes: false,
      deletions: false,
      conversations: false,
      restrict: { users: ["devi"], teams: ["maintainers"] },
    }),
  });
  const tagRules = ruleset(repo, {
    name: "Release tags",
    target: "tag",
    enforcement: "active",
    bypass_actors: [{ actor_id: 5, actor_type: "RepositoryRole", bypass_mode: "always" }],
    conditions: { ref_name: { include: ["refs/tags/v*"], exclude: [] } },
    rules: [
      { type: "deletion" },
      { type: "non_fast_forward" },
      {
        type: "tag_name_pattern",
        parameters: {
          operator: "regex",
          pattern: "^v\\d+\\.\\d+\\.\\d+(-rc\\.\\d+)?$",
          negate: false,
          name: "semver",
        },
      },
    ],
    created_at: "2026-03-01T10:00:00Z",
    updated_at: "2026-09-12T16:20:00Z",
  });
  const branchRules = ruleset(repo, {
    name: "Feature branch hygiene",
    target: "branch",
    enforcement: "evaluate",
    bypass_actors: [
      { actor_id: teams.maintainers.id, actor_type: "Team", bypass_mode: "pull_request" },
    ],
    conditions: {
      ref_name: { include: ["~ALL"], exclude: ["refs/heads/main", "refs/heads/release/*"] },
    },
    rules: [
      {
        type: "branch_name_pattern",
        parameters: {
          operator: "regex",
          pattern: "^[a-z]+/[a-z0-9-]+$",
          negate: false,
          name: "owner/topic",
        },
      },
      {
        type: "commit_message_pattern",
        parameters: { operator: "regex", pattern: "^[a-z]+: ", negate: false, name: "area prefix" },
      },
    ],
    created_at: "2026-08-20T09:30:00Z",
    updated_at: "2026-08-20T09:30:00Z",
  });
  s.rulesets = [tagRules, branchRules];
  s.ruleHistory.set(tagRules.id, [
    {
      version_id: 3,
      actor: { id: users.devi.id, type: "User" },
      updated_at: "2026-09-12T16:20:00Z",
    },
    {
      version_id: 2,
      actor: { id: users.mira.id, type: "User" },
      updated_at: "2026-05-02T11:00:00Z",
    },
    {
      version_id: 1,
      actor: { id: users.mira.id, type: "User" },
      updated_at: "2026-03-01T10:00:00Z",
    },
  ]);
  s.ruleHistory.set(branchRules.id, [
    {
      version_id: 1,
      actor: { id: users.lin.id, type: "User" },
      updated_at: "2026-08-20T09:30:00Z",
    },
  ]);
  s.suites = seedSuites(repo, tagRules, branchRules);
  s.autolinks = [
    {
      id: id(),
      key_prefix: "GY-",
      url_template: "https://linear.graftyard.dev/issue/GY-<num>",
      is_alphanumeric: false,
      updated_at: "2026-01-12T10:00:00Z",
    },
    {
      id: id(),
      key_prefix: "INC-",
      url_template: "https://status.graftyard.dev/incidents/<num>",
      is_alphanumeric: true,
      updated_at: "2026-04-03T15:00:00Z",
    },
  ];
  const ci = hook(repo, {
    url: "https://ci.graftyard.dev/hooks/github",
    events: ["push", "pull_request", "check_suite"],
    active: true,
    at: "2025-04-01T08:00:00Z",
  });
  const chat = hook(repo, {
    url: "https://chat.graftyard.dev/services/ghes/edge",
    events: ["*"],
    active: false,
    at: "2026-02-10T12:00:00Z",
  });
  s.hooks = [ci, chat];
  const ciDeliveries = [
    delivery(repo, ci, "push", null, "2026-11-08T10:12:05Z", 200),
    delivery(repo, ci, "pull_request", "synchronize", "2026-11-08T09:41:12Z", 200),
    delivery(repo, ci, "check_suite", "completed", "2026-11-08T09:20:44Z", 500),
    delivery(repo, ci, "pull_request", "opened", "2026-11-07T17:02:31Z", 200),
    delivery(repo, ci, "push", null, "2026-11-07T15:55:00Z", 0),
  ];
  s.deliveries.set(ci.id, ciDeliveries);
  ci.last_response = { code: 200, status: "active", message: "OK" };
  s.deliveries.set(chat.id, [
    delivery(repo, chat, "issues", "opened", "2026-10-20T08:00:00Z", 500),
  ]);
  chat.last_response = { code: 500, status: "failed", message: "Invalid HTTP Response: 500" };
  s.keys = [
    deployKey(
      repo,
      "ci-runner (read)",
      keyText("edge:ci"),
      true,
      "2025-05-12T08:00:00Z",
      "2026-11-08T10:13:00Z",
    ),
    deployKey(
      repo,
      "release-bot",
      keyText("edge:release"),
      false,
      "2026-03-03T10:00:00Z",
      "2026-10-30T18:44:00Z",
    ),
  ];
  s.lfs = "enabled";
  s.lfsBytes = 734_003_200;
  return s;
}

function seedSuites(repo: Repository, tags: Ruleset, branchesRuleset: Ruleset): RuleSuite[] {
  const rows: [string, string, RuleSuite["result"], RuleEvaluation[]][] = [
    [
      "refs/tags/v0.4.0-rc.1",
      "devi",
      "pass",
      [ev(tags, "active", "pass", "tag_name_pattern", null)],
    ],
    [
      "refs/heads/tomas/label-objects",
      "tomas",
      "pass",
      [ev(branchesRuleset, "evaluate", "pass", "branch_name_pattern", null)],
    ],
    [
      "refs/heads/Fix_Retry",
      "lin",
      "pass",
      [
        ev(
          branchesRuleset,
          "evaluate",
          "fail",
          "branch_name_pattern",
          "Branch name Fix_Retry does not match ^[a-z]+/[a-z0-9-]+$",
        ),
      ],
    ],
    [
      "refs/tags/v0.4",
      "tomas",
      "fail",
      [ev(tags, "active", "fail", "tag_name_pattern", "Tag name v0.4 does not match semver")],
    ],
    [
      "refs/tags/v0.3.0",
      "mira",
      "bypass",
      [
        ev(
          tags,
          "active",
          "fail",
          "deletion",
          "Deleting a matching tag; bypassed by repository admin",
        ),
      ],
    ],
    [
      "refs/heads/mira/idempotent-retry",
      "mira",
      "pass",
      [
        ev(
          branchesRuleset,
          "evaluate",
          "fail",
          "commit_message_pattern",
          "Commit 3f1c2a9 message does not start with an area prefix",
        ),
      ],
    ],
  ];
  return rows.map(([ref, actor, result, evaluations], i) => {
    const n = 90_000 + i;
    const who = findPerson(actor)!;
    return {
      id: n,
      actor_id: who.id,
      actor_name: who.login,
      before_sha: sha(`${ref}:before:${i}`),
      after_sha: sha(`${ref}:after:${i}`),
      ref,
      repository_id: repo.id,
      repository_name: repo.name,
      // Relative to page load, so the insights time filter always has rows in each window.
      pushed_at: new Date(Date.now() - (i * i + 1) * 3_600_000).toISOString(),
      result,
      evaluation_result: evaluations.some((e) => e.result === "fail") ? "fail" : "pass",
      evaluations,
    };
  });
}

function ev(
  r: Ruleset,
  enforcement: "active" | "evaluate",
  result: "pass" | "fail",
  rule_type: string,
  details: string | null,
): RuleEvaluation {
  return {
    rule_source: { type: "ruleset", id: r.id, name: r.name },
    enforcement,
    result,
    rule_type,
    details,
  };
}

function seedRaft(repo: Repository): AdminState {
  const s = empty(branches[repo.full_name]);
  s.collaborators = [{ user: users.lin, role: "admin" }];
  s.teams = [{ slug: "storage", role: "write" }];
  s.protections.set("main", {
    created_at: "2025-06-01T09:00:00Z",
    protection: protectionRecord(repo, "main", {
      reviews: { count: 1, dismissStale: false, codeOwners: false, lastPush: false },
      checks: null,
      enforceAdmins: false,
      signatures: false,
      linear: false,
      forcePushes: false,
      deletions: false,
      conversations: false,
      restrict: null,
    }),
  });
  s.keys = [
    deployKey(repo, "jepsen-harness", keyText("raft:jepsen"), true, "2026-07-01T09:00:00Z", null),
  ];
  return s;
}

function seedWeb(repo: Repository): AdminState {
  const s = empty(branches[repo.full_name]);
  s.collaborators = [{ user: users.lin, role: "write" }];
  s.teams = [{ slug: "maintainers", role: "admin" }];
  const preview = hook(repo, {
    url: "https://preview.graftyard.dev/deploy",
    events: ["pull_request"],
    active: true,
    at: "2026-09-01T10:00:00Z",
  });
  s.hooks = [preview];
  s.deliveries.set(preview.id, [
    delivery(repo, preview, "pull_request", "opened", "2026-11-06T09:01:00Z", 200),
  ]);
  preview.last_response = { code: 200, status: "active", message: "OK" };
  return s;
}

function seededForks(parent: Repository): Repository[] {
  const owners = [users.tomas, users.lin, users.devi];
  return owners.slice(0, parent.forks_count).map((owner, i) => {
    const r = repository(parent.name, parent.description ?? "", "public", {
      language: parent.language,
      stars: 0,
      topics: [],
      pushed: new Date(Date.parse("2026-10-20T12:00:00Z") - i * 9 * 86_400_000).toISOString(),
      owner,
    });
    r.fork = true;
    r.forks = r.forks_count = 0;
    r.created_at = new Date(Date.parse("2026-06-01T09:00:00Z") + i * 31 * 86_400_000).toISOString();
    return r;
  });
}

for (const r of db.repos()) {
  const seed =
    r.name === "edge" ? seedEdge : r.name === "raft" ? seedRaft : r.name === "web" ? seedWeb : null;
  if (seed && r.owner.login === "graftyard")
    states.set(r.id, { ...seed(r), seededForks: seededForks(r) });
}

// ---- Store API ----

export const admin = {
  // The admin record for a live repository; a new repository gets an empty one.
  state(owner: string, name: string): { repo: Repository; admin: AdminState } | undefined {
    const repo = db.state(owner, name)?.repo;
    if (!repo) return undefined;
    let s = states.get(repo.id);
    if (!s) states.set(repo.id, (s = empty([repo.default_branch])));
    return { repo, admin: s };
  },
  // A repository this area created: its record starts with the given branches.
  create(repo: Repository, init: Partial<AdminState>) {
    states.set(repo.id, { ...empty(), ...init });
  },
  drop(repoId: number) {
    states.delete(repoId);
  },
  rules(s: AdminState): ProtectionRule[] {
    const names = s.branches;
    return [...s.protections].map(([pattern, { protection, created_at }]) => ({
      pattern,
      protection,
      created_at,
      matching_branches: names.filter((b) => matchesPattern(pattern, b)),
    }));
  },
  // The rule that covers a concrete branch name, exact patterns first, as GitHub picks.
  ruleFor(s: AdminState, branch: string) {
    const exact = s.protections.get(branch);
    if (exact) return { pattern: branch, ...exact };
    for (const [pattern, rec] of s.protections) {
      if (matchesPattern(pattern, branch)) return { pattern, ...rec };
    }
    return undefined;
  },
  // Every key on every repository: GHES rejects a deploy key another repository or user
  // already holds.
  keyInUse(key: string): boolean {
    const body = key.trim().split(/\s+/).slice(0, 2).join(" ");
    for (const s of states.values()) {
      if (s.keys.some((k) => k.key.split(/\s+/).slice(0, 2).join(" ") === body)) return true;
    }
    return false;
  },
  // Check names seen on the repository, for the required-checks autocomplete.
  checkNames(repo: Repository): string[] {
    const state = db.state(repo.owner.login, repo.name);
    const seen = new Set<string>([
      "test (ubuntu-latest)",
      "test (macos-latest)",
      "clippy",
      "rustfmt",
      "build",
      "ci/deploy-preview",
    ]);
    for (const pr of state?.pulls ?? [])
      for (const run of db.checkRuns(pr.head.sha)) seen.add(run.name);
    return [...seen].sort();
  },
  installationsFor(repo: Repository): Installation[] {
    return installations
      .filter(
        (i) =>
          i.repos === "all" ||
          (repo.owner.login === "graftyard" && i.repos.includes(repo.full_name)),
      )
      .map((i) => i.install);
  },
};

export { keyText };
