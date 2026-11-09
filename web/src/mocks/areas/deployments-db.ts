import type { Schemas } from "@/api/client";
import {
  HOST,
  branchSha,
  branches,
  edge,
  findRepo,
  id,
  raft,
  sha,
  tags,
  teamMembers,
  teams,
  users,
  type Repository,
  type SimpleUser,
} from "../world";
import { releasesDb } from "./releases-db";

// Deployments, their statuses, environments with their protection rules, branch policies,
// secrets, and variables, and the pending reviews a protected environment holds. A new
// deployment runs on timers: queued, then in_progress, then success, so polling pages see
// each state arrive.

export type Deployment = Schemas["deployment"];
export type DeploymentStatus = Schemas["deployment-status"];
export type Environment = Schemas["environment"];
export type BranchPolicy = Schemas["deployment-branch-policy"];
type StatusState = DeploymentStatus["state"];
type Team = Schemas["team"];
type Reviewer = { type: "User"; reviewer: SimpleUser } | { type: "Team"; reviewer: Team };

// The settings an environment's PUT writes. The served `environment` is built from these.
export type EnvSettings = {
  id: number;
  name: string;
  created_at: string;
  updated_at: string;
  wait_timer: number;
  prevent_self_review: boolean;
  reviewers: Reviewer[];
  branch_policy: { protected_branches: boolean; custom_branch_policies: boolean } | null;
  policies: BranchPolicy[];
  secrets: Schemas["actions-secret"][];
  variables: Schemas["actions-variable"][];
};

// Not in GHES REST: the deployments a protected environment holds for review. The run-level
// endpoint belongs to Actions, so this mock keeps its own list, served on the private API.
export type PendingReview = Schemas["pending-deployment"] & { deployment: Deployment };

type RepoDeployments = {
  repo: Repository;
  environments: EnvSettings[];
  deployments: Deployment[];
  statuses: Map<number, DeploymentStatus[]>;
  pending: PendingReview[];
};

const FINAL: StatusState[] = ["success", "failure", "error", "inactive"];
export const isFinal = (s: StatusState | undefined) => s !== undefined && FINAL.includes(s);

function env(
  repo: Repository,
  name: string,
  s: Partial<Omit<EnvSettings, "id" | "name" | "created_at" | "updated_at">> & { at: string },
): EnvSettings {
  return {
    id: parseInt(sha(`${repo.full_name}:env:${name}`).slice(0, 7), 16),
    name,
    created_at: s.at,
    updated_at: s.at,
    wait_timer: s.wait_timer ?? 0,
    prevent_self_review: s.prevent_self_review ?? false,
    reviewers: s.reviewers ?? [],
    branch_policy: s.branch_policy ?? null,
    policies: s.policies ?? [],
    secrets: s.secrets ?? [],
    variables: s.variables ?? [],
  };
}

let policyId = 5000;
function policy(name: string, type: "branch" | "tag" = "branch"): BranchPolicy {
  const n = policyId++;
  return { id: n, node_id: `MDE2OkdhdGVCcmFuY2hQb2xpY3k${n}`, name, type };
}

export function serveEnvironment(repo: Repository, e: EnvSettings): Environment {
  const rule = (kind: string) => ({
    id: parseInt(sha(`${e.id}:${kind}`).slice(0, 7), 16),
    node_id: `GA_${sha(`${e.id}:${kind}`).slice(0, 12)}`,
    type: kind,
  });
  const rules: NonNullable<Environment["protection_rules"]> = [];
  if (e.wait_timer > 0) rules.push({ ...rule("wait_timer"), wait_timer: e.wait_timer });
  if (e.reviewers.length > 0) {
    rules.push({
      ...rule("required_reviewers"),
      prevent_self_review: e.prevent_self_review,
      reviewers: e.reviewers,
    });
  }
  if (e.branch_policy) rules.push(rule("branch_policy"));
  return {
    id: e.id,
    node_id: `EN_${sha(`${e.id}`).slice(0, 12)}`,
    name: e.name,
    url: `${repo.url}/environments/${encodeURIComponent(e.name)}`,
    html_url: `${repo.html_url}/deployments/activity_log?environments_filter=${encodeURIComponent(e.name)}`,
    created_at: e.created_at,
    updated_at: e.updated_at,
    protection_rules: rules,
    deployment_branch_policy: e.branch_policy,
  };
}

function deployment(
  repo: Repository,
  s: {
    ref: string;
    environment: string;
    creator: SimpleUser;
    at: string;
    description?: string;
    task?: string;
    production?: boolean;
    transient?: boolean;
  },
): Deployment {
  const n = id();
  const url = `${repo.url}/deployments/${n}`;
  const isTag = (tags[repo.full_name] ?? []).includes(s.ref);
  return {
    url,
    id: n,
    node_id: `DE_${sha(url).slice(0, 12)}`,
    sha: branchSha(repo.full_name, isTag ? `tag:${s.ref}` : s.ref),
    ref: s.ref,
    task: s.task ?? "deploy",
    payload: {},
    original_environment: s.environment,
    environment: s.environment,
    description: s.description ?? null,
    creator: s.creator,
    created_at: s.at,
    updated_at: s.at,
    statuses_url: `${url}/statuses`,
    repository_url: repo.url,
    transient_environment: s.transient ?? false,
    production_environment: s.production ?? s.environment === "production",
  };
}

let statusId = 70_000;
function status(
  repo: Repository,
  d: Deployment,
  state: StatusState,
  at: string,
  s: { by?: SimpleUser; description?: string; url?: string } = {},
): DeploymentStatus {
  const n = statusId++;
  return {
    url: `${d.url}/statuses/${n}`,
    id: n,
    node_id: `DES_${sha(`${d.id}:${n}`).slice(0, 12)}`,
    state,
    creator: s.by ?? d.creator,
    description: s.description ?? "",
    environment: d.environment,
    target_url: `${repo.html_url}/deployments/${d.id}`,
    created_at: at,
    updated_at: at,
    deployment_url: d.url,
    repository_url: repo.url,
    environment_url: s.url ?? "",
    log_url: `${repo.html_url}/deployments/${d.id}/log`,
  };
}

function seedRepo(repo: Repository, environments: EnvSettings[]): RepoDeployments {
  return { repo, environments, deployments: [], statuses: new Map(), pending: [] };
}

const store = new Map<string, RepoDeployments>();

function push(
  r: RepoDeployments,
  d: Deployment,
  history: [StatusState, string, { by?: SimpleUser; description?: string; url?: string }?][],
) {
  r.deployments.push(d);
  // Statuses are kept newest first, as GHES lists them.
  r.statuses.set(
    d.id,
    history.map(([state, at, extra]) => status(r.repo, d, state, at, extra)).reverse(),
  );
  d.updated_at = history.at(-1)?.[1] ?? d.created_at;
}

{
  const at = "2025-06-01T09:00:00Z";
  const r = seedRepo(edge, [
    env(edge, "production", {
      at,
      prevent_self_review: true,
      reviewers: [
        { type: "Team", reviewer: teams.maintainers },
        { type: "User", reviewer: users.devi },
      ],
      branch_policy: { protected_branches: false, custom_branch_policies: true },
      policies: [policy("main"), policy("release/*"), policy("v*", "tag")],
      secrets: [
        { name: "CLOUDFLARE_API_TOKEN", created_at: at, updated_at: "2026-09-12T10:00:00Z" },
        { name: "SIGNING_KEY", created_at: at, updated_at: at },
      ],
      variables: [
        {
          name: "WORKER_ROUTE",
          value: "edge.graftyard.dev/*",
          created_at: at,
          updated_at: at,
        },
        { name: "REGION_HINT", value: "weur", created_at: at, updated_at: at },
      ],
    }),
    env(edge, "staging", {
      at,
      wait_timer: 1,
      branch_policy: { protected_branches: false, custom_branch_policies: true },
      policies: [policy("main"), policy("mira/*"), policy("devi/*"), policy("lin/*")],
      secrets: [{ name: "CLOUDFLARE_API_TOKEN", created_at: at, updated_at: at }],
      variables: [
        {
          name: "WORKER_ROUTE",
          value: "staging.edge.graftyard.dev/*",
          created_at: at,
          updated_at: at,
        },
      ],
    }),
    env(edge, "preview", { at: "2026-10-01T09:00:00Z" }),
  ]);
  const prod = "https://edge.graftyard.dev";
  const stage = "https://staging.edge.graftyard.dev";
  push(
    r,
    deployment(edge, {
      ref: "v0.2.0",
      environment: "production",
      creator: users.devi,
      at: "2025-09-15T15:00:00Z",
      description: "Release v0.2.0",
    }),
    [
      ["queued", "2025-09-15T15:00:00Z"],
      ["in_progress", "2025-09-15T15:01:10Z"],
      ["success", "2025-09-15T15:03:40Z", { url: prod }],
      ["inactive", "2026-10-30T16:40:00Z", { description: "Superseded by a newer deployment" }],
    ],
  );
  push(
    r,
    deployment(edge, {
      ref: "v0.3.0",
      environment: "production",
      creator: users.devi,
      at: "2026-10-30T16:30:00Z",
      description: "Release v0.3.0",
    }),
    [
      ["queued", "2026-10-30T16:30:00Z"],
      ["in_progress", "2026-10-30T16:36:00Z", { by: users.mira, description: "Approved by @mira" }],
      ["success", "2026-10-30T16:40:00Z", { url: prod }],
    ],
  );
  push(
    r,
    deployment(edge, {
      ref: "main",
      environment: "staging",
      creator: users.tomas,
      at: "2026-11-05T10:00:00Z",
      description: "Nightly",
    }),
    [
      ["queued", "2026-11-05T10:00:00Z"],
      ["in_progress", "2026-11-05T10:01:00Z"],
      ["failure", "2026-11-05T10:04:30Z", { description: "Smoke test GET /meta answered 500" }],
    ],
  );
  push(
    r,
    deployment(edge, {
      ref: "main",
      environment: "staging",
      creator: users.tomas,
      at: "2026-11-07T10:00:00Z",
      description: "Nightly",
    }),
    [
      ["queued", "2026-11-07T10:00:00Z"],
      ["in_progress", "2026-11-07T10:01:00Z"],
      ["success", "2026-11-07T10:03:20Z", { url: stage }],
    ],
  );
  push(
    r,
    deployment(edge, {
      ref: "lin/learner-reads",
      environment: "preview",
      creator: users.lin,
      at: "2026-11-07T15:00:00Z",
      description: "Preview for #9",
      transient: true,
    }),
    [
      ["queued", "2026-11-07T15:00:00Z"],
      ["in_progress", "2026-11-07T15:00:30Z"],
      ["error", "2026-11-07T15:02:00Z", { description: "Container image failed to build" }],
    ],
  );
  const waiting = deployment(edge, {
    ref: "v0.4.0-rc.1",
    environment: "production",
    creator: users.tomas,
    at: "2026-11-08T08:30:00Z",
    description: "Release candidate v0.4.0-rc.1",
  });
  push(r, waiting, [["queued", "2026-11-08T08:30:00Z", { description: "Waiting for review" }]]);
  const prodEnv = r.environments[0];
  r.pending.push(pendingFor(r, prodEnv, waiting));
  store.set("graftyard/edge", r);
}
{
  const at = "2026-09-01T08:00:00Z";
  const r = seedRepo(raft, [
    env(raft, "production", {
      at,
      reviewers: [{ type: "Team", reviewer: teams.storage }],
      branch_policy: { protected_branches: true, custom_branch_policies: false },
    }),
  ]);
  push(
    r,
    deployment(raft, {
      ref: "v0.1.0",
      environment: "production",
      creator: users.lin,
      at: "2026-09-01T09:00:00Z",
    }),
    [
      ["queued", "2026-09-01T09:00:00Z"],
      ["in_progress", "2026-09-01T09:05:00Z", { by: users.devi, description: "Approved by @devi" }],
      ["success", "2026-09-01T09:07:00Z", { url: "https://raft.graftyard.dev" }],
    ],
  );
  store.set("graftyard/raft", r);
}

function pendingFor(r: RepoDeployments, e: EnvSettings, d: Deployment): PendingReview {
  const served = serveEnvironment(r.repo, e);
  return {
    deployment: d,
    environment: {
      id: served.id,
      node_id: served.node_id,
      name: served.name,
      url: served.url,
      html_url: served.html_url,
    },
    wait_timer: e.wait_timer,
    wait_timer_started_at: e.wait_timer > 0 ? d.created_at : null,
    current_user_can_approve: canApprove(e, d),
    reviewers: e.reviewers,
  };
}

// The viewer may review when a reviewer rule names them or a team they are on, and, under
// prevent-self-review, only when someone else created the deployment.
function canApprove(e: EnvSettings, d: Deployment) {
  const viewer = users.mira.login;
  const named = e.reviewers.some((r) =>
    r.type === "User"
      ? r.reviewer.login === viewer
      : (teamMembers[r.reviewer.slug as keyof typeof teams] ?? []).some((u) => u.login === viewer),
  );
  return named && !(e.prevent_self_review && d.creator?.login === viewer);
}

// ---- Store API the handlers use ----

function state(owner: string, name: string): RepoDeployments | undefined {
  const key = `${owner}/${name}`.toLowerCase();
  let s = store.get(key);
  const repo = findRepo(owner, name);
  if (!s && repo) store.set(key, (s = seedRepo(repo, [])));
  return s;
}

// A glob as GHES matches deployment branch policies: `*` stays within one path segment.
function globMatch(pattern: string, value: string) {
  const re = pattern
    .split("*")
    .map((p) => p.replace(/[.+?^${}()|[\]\\]/g, "\\$&"))
    .join("[^/]*");
  return new RegExp(`^${re}$`).test(value);
}

const PROTECTED = new Set(["main", "release/0.4"]);

const latestStatus = (r: RepoDeployments, deploymentId: number) =>
  r.statuses.get(deploymentId)?.[0];

const timers = new Map<number, ReturnType<typeof setTimeout>[]>();
function later(deploymentId: number, ms: number, fn: () => void) {
  const list = timers.get(deploymentId) ?? [];
  list.push(setTimeout(fn, ms));
  timers.set(deploymentId, list);
}
function stopTimers(deploymentId: number) {
  for (const t of timers.get(deploymentId) ?? []) clearTimeout(t);
  timers.delete(deploymentId);
}

export const deploymentsDb = {
  state,
  isFinal,
  environment: (r: RepoDeployments, name: string) =>
    r.environments.find((e) => e.name.toLowerCase() === name.toLowerCase()),
  serve: (r: RepoDeployments, e: EnvSettings) => serveEnvironment(r.repo, e),
  statuses: (r: RepoDeployments, deploymentId: number) => r.statuses.get(deploymentId) ?? [],
  latestStatus,
  reviewerUser: (login: string) => Object.values(users).find((u) => u.login === login),
  reviewerById(reviewerId: number, type: "User" | "Team"): Reviewer | undefined {
    if (type === "User") {
      const u = Object.values(users).find((x) => x.id === reviewerId && x.type === "User");
      return u && { type, reviewer: u };
    }
    const t = Object.values(teams).find((x) => x.id === reviewerId);
    return t && { type, reviewer: t };
  },

  upsertEnvironment(r: RepoDeployments, name: string): EnvSettings {
    const existing = this.environment(r, name);
    if (existing) return existing;
    const created = env(r.repo, name, { at: new Date().toISOString() });
    r.environments.push(created);
    return created;
  },

  deleteEnvironment(r: RepoDeployments, name: string) {
    const e = this.environment(r, name);
    if (!e) return;
    r.environments = r.environments.filter((x) => x !== e);
    r.pending = r.pending.filter((p) => p.environment.name !== e.name);
  },

  // Whether a ref may deploy to an environment under its branch policy.
  refAllowed(r: RepoDeployments, e: EnvSettings | undefined, ref: string) {
    if (!e?.branch_policy) return true;
    const isTag = releasesDb.tags(releasesDb.state(r.repo.owner.login, r.repo.name)!).includes(ref);
    if (e.branch_policy.protected_branches) return !isTag && PROTECTED.has(ref);
    return e.policies.some((p) => (p.type === "tag") === isTag && globMatch(p.name!, ref));
  },

  refExists(r: RepoDeployments, ref: string) {
    const rel = releasesDb.state(r.repo.owner.login, r.repo.name)!;
    return (
      (branches[r.repo.full_name] ?? []).includes(ref) ||
      releasesDb.tags(rel).includes(ref) ||
      /^[0-9a-f]{7,40}$/.test(ref)
    );
  },

  create(
    r: RepoDeployments,
    input: {
      ref: string;
      environment?: string;
      description?: string | null;
      task?: string;
      production_environment?: boolean;
      transient_environment?: boolean;
    },
  ): Deployment {
    const now = new Date().toISOString();
    const name = input.environment || "production";
    // GHES creates an environment the first time a deployment names it.
    const e = this.upsertEnvironment(r, name);
    const d = deployment(r.repo, {
      ref: input.ref,
      environment: e.name,
      creator: users.mira,
      at: now,
      description: input.description ?? undefined,
      task: input.task,
      production: input.production_environment,
      transient: input.transient_environment,
    });
    r.deployments.push(d);
    r.statuses.set(d.id, []);
    if (e.reviewers.length > 0) {
      this.addStatus(r, d, "queued", { description: "Waiting for review" });
      r.pending.push(pendingFor(r, e, d));
    } else {
      this.run(r, d, e.wait_timer);
    }
    return d;
  },

  // queued, then in_progress, then success, with the wait timer (minutes) first if any.
  // A mock minute is five seconds, so a wait timer shows its countdown without stalling.
  run(r: RepoDeployments, d: Deployment, waitMinutes: number) {
    const wait = waitMinutes * 5000;
    this.addStatus(r, d, "queued", {
      description: waitMinutes > 0 ? `Waiting ${waitMinutes} min before deploying` : "Queued",
    });
    later(d.id, wait + 1500, () =>
      this.addStatus(r, d, "in_progress", { description: "Deploying" }),
    );
    later(d.id, wait + 5000, () => {
      const host = d.environment === "production" ? "" : `${d.environment}.`;
      this.addStatus(r, d, "success", {
        description: "Deployed",
        url: `https://${host}${r.repo.name}.graftyard.dev`,
      });
    });
  },

  addStatus(
    r: RepoDeployments,
    d: Deployment,
    state: StatusState,
    s: { description?: string; url?: string; by?: SimpleUser; autoInactive?: boolean } = {},
  ): DeploymentStatus {
    const now = new Date().toISOString();
    const list = r.statuses.get(d.id) ?? [];
    const st = status(r.repo, d, state, now, { by: s.by ?? users.mira, ...s });
    r.statuses.set(d.id, [st, ...list]);
    d.updated_at = now;
    if (isFinal(state)) stopTimers(d.id);
    // A success makes every earlier non-transient deployment to the environment inactive.
    if (state === "success" && s.autoInactive !== false) {
      for (const other of r.deployments) {
        if (other.id === d.id || other.environment !== d.environment) continue;
        if (latestStatus(r, other.id)?.state === "success") {
          const prev = r.statuses.get(other.id) ?? [];
          r.statuses.set(other.id, [
            status(r.repo, other, "inactive", now, {
              by: users.mira,
              description: "Superseded by a newer deployment",
            }),
            ...prev,
          ]);
          other.updated_at = now;
        }
      }
    }
    if (state === "inactive" || state === "failure" || state === "error") {
      r.pending = r.pending.filter((p) => p.deployment.id !== d.id);
    }
    return st;
  },

  pendingFor: (r: RepoDeployments, deploymentId: number) =>
    r.pending.find((p) => p.deployment.id === deploymentId),

  // Approval: the review leaves the pending list and the deployment runs.
  approve(r: RepoDeployments, d: Deployment, comment: string) {
    r.pending = r.pending.filter((p) => p.deployment.id !== d.id);
    const e = this.environment(r, d.environment);
    const st = this.addStatus(r, d, "in_progress", {
      description: comment ? `Approved by @mira: ${comment}` : "Approved by @mira",
    });
    later(d.id, (e?.wait_timer ?? 0) * 5000 + 4000, () =>
      this.addStatus(r, d, "success", {
        description: "Deployed",
        url: `https://${d.environment === "production" ? "" : `${d.environment}.`}${r.repo.name}.graftyard.dev`,
      }),
    );
    return st;
  },

  remove(r: RepoDeployments, deploymentId: number) {
    stopTimers(deploymentId);
    r.deployments = r.deployments.filter((d) => d.id !== deploymentId);
    r.statuses.delete(deploymentId);
    r.pending = r.pending.filter((p) => p.deployment.id !== deploymentId);
  },

  newPolicy: policy,
  secretKey: (r: RepoDeployments, e: EnvSettings): Schemas["actions-public-key"] => ({
    key_id: sha(`${r.repo.full_name}:${e.name}:key`).slice(0, 20),
    // A 32-byte X25519 public key, base64. The mock never decrypts what it is sent.
    key: btoa(
      String.fromCharCode(
        ...sha(`${e.id}`)
          .match(/../g)!
          .map((h) => parseInt(h, 16)),
        ...Array(12).fill(7),
      ),
    ),
  }),
  host: HOST,
};
