import createClient from "openapi-fetch";
import { ApiError, waitForMocks, type Schemas } from "@/api/client";

// Graftyard's own API under /api/graftyard/v1, hand-typed from design/AGENT-LAYER.md,
// "API reference". GHES 3.22 serves no /api/graftyard/ path, so these types are not in
// ghes.d.ts. Field names, enums, status codes, and error bodies follow that YAML; where the
// YAML names a body without a shape, the comment says what was inferred.

type User = Schemas["simple-user"];
type App = NonNullable<Schemas["integration"]>;

// ---- Objects ----

export const TASK_STATES = [
  "queued",
  "in_progress",
  "waiting_for_user",
  "completed",
  "failed",
  "timed_out",
  "cancelled",
  "idle",
] as const;
export type TaskState = (typeof TASK_STATES)[number];

export type TrustView = {
  visible: string;
  hidden: {
    html_comments: number;
    invisible_chars: number;
    link_title_text: number;
    collapsed: number;
  };
  author: {
    association: Schemas["author-association"];
    can_push: boolean;
    is_bot: boolean;
    is_agent: boolean;
    first_contribution: boolean;
    trusted: boolean;
  };
};

export type Lease = {
  id: string;
  holder: {
    actor: { type: "User" | "App"; id: number };
    app_id: number | null;
    session: string | null;
  };
  paths: string[];
  refs: string[];
  mode: "shared" | "exclusive";
  plan_block: boolean;
  intent: TrustView;
  granted_at: string;
  expires_at: string;
};

export type Task = {
  // A lowercase UUID for a graftyard_run task, `<app_id>.<name>` for an external one.
  id: string;
  kind: "graftyard_run" | "external";
  state: TaskState;
  app_id: number;
  installation_id: number;
  requester: User | null;
  creator: User | App;
  issue_number: number | null;
  // Omitted without `contents: read`.
  prompt?: string | null;
  pr_number: number | null;
  summary: string;
  prefix: string | null;
  // Single GET only.
  leases?: Lease[];
  html_url: string;
  created_at: string;
  updated_at: string;
};

export type Policy = {
  ci: "approve" | "auto_read_only";
  labels: string[];
  extra_paths: string[];
  extra_human_approvals: number;
  leases: "advisory" | "agents_exclusive";
};

export type OrgAgents = {
  agents_enabled: boolean;
  workspace: "fork" | "in_repo";
  // Read only: always `<org>-agents`.
  readonly workspace_org: string;
  code_mode: boolean;
};

export type Grant = {
  id: string;
  client: string;
  redirect_hosts: string[];
  generation: number;
  created_at: string;
};

export type Verdict = "clean" | "path_overlap" | "conflict" | "stale";

export type ActivityEvent =
  | { type: "lease"; change: "acquired" | "released" | "expired"; lease: Lease }
  | { type: "task"; task_id: string; state: TaskState }
  | { type: "agent_push"; ref: string; head_sha: string; paths: string[] }
  | { type: "verdict"; ref: string; verdict: Verdict; files?: string[] }
  | {
      type: "agent_pr";
      number: number;
      action: "opened" | "readied" | "closed" | "merged";
      title: TrustView;
    };

export type StepKind = "model" | "tool" | "test" | "push";

// SessionLog schema 1 (design/AGENT-LAYER.md, Shape, `SessionLog`, `Step`, `Harness`).
export type SessionLog = {
  schema: 1;
  id: string;
  issue_snapshot: TrustView;
  plan: string;
  steps: {
    at: string;
    kind: StepKind;
    tool: string | null;
    input: string;
    output: string;
    truncated: boolean;
  }[];
  pushes: string[];
  prs: number[];
  harness: { name: string; version: string; model: string };
};
export type Step = SessionLog["steps"][number];

export type AgentAppKind = "graftyard_run" | "third_party" | "mcp";
export type AgentAppRow = {
  app_id: number;
  slug: string;
  kind: AgentAppKind;
  confine: string | null;
};

export type ReleaseEntry = { index: number; entry: string };

export type GithubError = {
  message: string;
  documentation_url: string;
  status?: string;
  errors?: {
    resource: string;
    field: string;
    code: "missing_field" | "invalid" | "already_exists";
  }[];
};
export type TaskConflict = GithubError & { state: TaskState };
export type LeaseConflict = GithubError & { conflicts: Lease[] };
export type FeedGone = GithubError & { oldest_cursor: string };
export type GrantRevoked = GithubError & { revoked: true };

// ---- openapi-typescript's shape ----

type Method = "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace";
type Headers = { headers: { [name: string]: unknown } };
type Json<T> = Headers & { content: { "application/json": T } };
type Empty = Headers & { content?: never };
type Body<T> = { content: { "application/json": T } };
type RepoPath = { owner: string; repo: string };
type Params<P, Q = never, H = never> = {
  query?: [Q] extends [never] ? never : Q;
  header?: [H] extends [never] ? never : H;
  path: P;
  cookie?: never;
};
// Methods a path does not serve are `?: never`, as openapi-typescript writes them.
type Item<P, Ops extends Partial<Record<Method, unknown>>> = Ops & {
  [M in Exclude<Method, keyof Ops>]?: never;
} & { parameters: { query?: never; header?: never; path?: P; cookie?: never } };

type TaskPath = RepoPath & { id: string };
type LeasePath = RepoPath & { id: string };

export type GraftyardPaths = {
  "/repos/{owner}/{repo}/tasks": Item<
    RepoPath,
    {
      post: {
        parameters: Params<RepoPath>;
        requestBody: Body<{
          issue_number?: number;
          // At most 64 KiB.
          prompt?: string;
          kind?: "external";
          // External only, and required there.
          session?: string;
          // External only.
          requester?: string;
          summary?: string;
        }>;
        responses: { 201: Json<Task>; 403: Json<GithubError>; 422: Json<GithubError> };
      };
      get: {
        parameters: Params<
          RepoPath,
          {
            state?: TaskState;
            app_id?: number;
            requester?: string;
            issue_number?: number;
            per_page?: number;
            after?: string;
          }
        >;
        requestBody?: never;
        responses: { 200: Json<Task[]> };
      };
    }
  >;
  "/repos/{owner}/{repo}/tasks/{id}": Item<
    TaskPath,
    {
      get: {
        parameters: Params<TaskPath>;
        requestBody?: never;
        responses: { 200: Json<Task>; 404: Json<GithubError> };
      };
      patch: {
        parameters: Params<TaskPath>;
        requestBody: Body<{ state?: TaskState; pr_number?: number; summary?: string }>;
        responses: {
          200: Json<Task>;
          403: Json<GithubError>;
          409: Json<TaskConflict>;
          422: Json<GithubError>;
        };
      };
    }
  >;
  "/repos/{owner}/{repo}/tasks/{id}/cancel": Item<
    TaskPath,
    {
      post: {
        parameters: Params<TaskPath>;
        requestBody?: never;
        responses: { 200: Json<Task>; 409: Json<TaskConflict> };
      };
    }
  >;
  "/repos/{owner}/{repo}/tasks/{id}/log": Item<
    TaskPath,
    {
      get: {
        parameters: Params<TaskPath>;
        requestBody?: never;
        // 404 until the log is pushed.
        responses: { 200: Json<SessionLog>; 404: Json<GithubError> };
      };
    }
  >;
  "/repos/{owner}/{repo}/leases": Item<
    RepoPath,
    {
      post: {
        parameters: Params<RepoPath, never, { "X-Graftyard-Agent-Session"?: string }>;
        requestBody: Body<{
          // 1 to 32 globs.
          paths: string[];
          // At most 2.
          refs?: string[];
          mode?: "shared" | "exclusive";
          // 60 to 3600, default 600.
          ttl_seconds?: number;
          // At most 280 bytes.
          intent?: string;
          session?: string;
        }>;
        responses: {
          201: Json<{ lease: Lease; overlaps: Lease[] }>;
          409: Json<LeaseConflict>;
          422: Json<GithubError>;
        };
      };
      get: {
        parameters: Params<RepoPath, { paths?: string[]; per_page?: number; after?: string }>;
        requestBody?: never;
        responses: { 200: Json<Lease[]> };
      };
    }
  >;
  "/repos/{owner}/{repo}/leases/{id}/renew": Item<
    LeasePath,
    {
      post: {
        parameters: Params<LeasePath>;
        requestBody: Body<{ ttl_seconds?: number }>;
        responses: { 200: Json<Lease>; 404: Json<GithubError> };
      };
    }
  >;
  "/repos/{owner}/{repo}/leases/{id}": Item<
    LeasePath,
    {
      delete: {
        parameters: Params<LeasePath>;
        requestBody?: never;
        responses: { 204: Empty };
      };
    }
  >;
  "/repos/{owner}/{repo}/activity": Item<
    RepoPath,
    {
      get: {
        parameters: Params<RepoPath, { cursor?: string; wait?: number; paths?: string[] }>;
        requestBody?: never;
        responses: {
          200: Json<{ cursor: string; events: ActivityEvent[] }>;
          410: Json<FeedGone>;
          429: Json<GithubError>;
        };
      };
    }
  >;
  "/repos/{owner}/{repo}/workspace": Item<
    RepoPath,
    {
      post: {
        parameters: Params<RepoPath>;
        requestBody?: never;
        responses: {
          201: Json<{ full_name: string; created: true }>;
          200: Json<{ full_name: string; created: false }>;
          422: Json<GithubError>;
        };
      };
    }
  >;
  "/repos/{owner}/{repo}/insights": Item<
    RepoPath,
    {
      post: {
        parameters: Params<RepoPath>;
        requestBody: Body<{
          ref: string;
          head_sha: string;
          checks: ("overlap" | "provenance")[];
          cluster?: number;
        }>;
        // The YAML says "written" and "unchanged" without a body; the echo is [inference].
        responses: {
          201: Json<{ ref: string; head_sha: string; written: true }>;
          200: Json<{ ref: string; head_sha: string; written: false }>;
          409: Json<GithubError>;
        };
      };
    }
  >;
  "/repos/{owner}/{repo}/agents/policy": Item<
    RepoPath,
    {
      get: {
        parameters: Params<RepoPath>;
        requestBody?: never;
        responses: { 200: Json<Policy> };
      };
      put: {
        parameters: Params<RepoPath>;
        requestBody: Body<Policy>;
        responses: { 200: Json<Policy> };
      };
    }
  >;
  "/orgs/{org}/agents": Item<
    { org: string },
    {
      get: {
        parameters: Params<{ org: string }>;
        requestBody?: never;
        responses: { 200: Json<OrgAgents> };
      };
      patch: {
        parameters: Params<{ org: string }>;
        requestBody: Body<Partial<Omit<OrgAgents, "workspace_org">>>;
        responses: { 200: Json<OrgAgents>; 422: Json<GithubError> };
      };
    }
  >;
  "/agents/apps": Item<
    never,
    {
      get: {
        parameters: { query?: never; header?: never; path?: never; cookie?: never };
        requestBody?: never;
        responses: { 200: Json<AgentAppRow[]> };
      };
    }
  >;
  "/agents/apps/{app_id}": Item<
    { app_id: number },
    {
      put: {
        parameters: Params<{ app_id: number }>;
        requestBody: Body<{ kind: AgentAppKind; confine?: string | null }>;
        responses: { 200: Json<AgentAppRow>; 422: Json<GithubError> };
      };
      delete: {
        parameters: Params<{ app_id: number }>;
        requestBody?: never;
        responses: { 204: Empty };
      };
    }
  >;
  "/mcp/grants": Item<
    never,
    {
      post: {
        parameters: { query?: never; header?: never; path?: never; cookie?: never };
        requestBody: Body<{ client: string; redirect_hosts: string[] }>;
        responses: { 201: Json<Grant> };
      };
      get: {
        parameters: { query?: never; header?: never; path?: never; cookie?: never };
        requestBody?: never;
        responses: { 200: Json<Grant[]> };
      };
    }
  >;
  "/mcp/grants/{id}/generation": Item<
    { id: string },
    {
      post: {
        parameters: Params<{ id: string }>;
        requestBody: Body<{ generation: number }>;
        responses: { 200: Json<Grant>; 409: Json<GrantRevoked> };
      };
    }
  >;
  "/mcp/grants/{id}": Item<
    { id: string },
    {
      delete: {
        parameters: Params<{ id: string }>;
        requestBody?: never;
        responses: { 204: Empty };
      };
    }
  >;
  "/release/entries": Item<
    never,
    {
      get: {
        parameters: {
          query?: { after?: number; per_page?: number };
          header?: never;
          path?: never;
          cookie?: never;
        };
        requestBody?: never;
        responses: { 200: Json<ReleaseEntry[]> };
      };
    }
  >;
};

export const GRAFTYARD_BASE = "/api/graftyard/v1";
export const gy = createClient<GraftyardPaths>({ baseUrl: GRAFTYARD_BASE });
gy.use(waitForMocks);

// ---- The opt-in agent media type on two GHES reads ----

// design/AGENT-LAYER.md, "Trust view, overlaps, provenance": the media type adds these
// fields and `Vary: Accept`; every other client sees GHES's bytes. Their inner shapes are
// not in the API reference and follow `AgentMeta`, `InsightCheck`, and `Source` [inference].
export const AGENT_MEDIA_TYPE = "application/vnd.graftyard.agent+json";

export type ProvenanceSource =
  | {
      type: "claimed";
      key:
        | "graftyard_session"
        | "claude_session"
        | "amp_thread_id"
        | "co_authored_by"
        | "assisted_by";
      value: string;
    }
  | { type: "ai_note"; note: string }
  | { type: "session"; session: string };

// The edge serves graftyard_trust today; the other fields arrive with the agent layer's
// later work packages, so a reply may leave them out.
export type AgentPullFields = {
  graftyard_trust: TrustView;
  graftyard_agent?: { app_id: number; session: string | null; requester: User | null } | null;
  graftyard_overlaps?: {
    with: { ref: string; pr_number: number | null; verdict: Verdict }[];
    same_issue: number[];
  } | null;
  graftyard_provenance?: { sessions: ProvenanceSource[] } | null;
};
export type AgentPull = Schemas["pull-request"] & AgentPullFields;
export type AgentIssue = Schemas["issue"] & { graftyard_trust: TrustView };

type PullPath = RepoPath & { pull_number: number };
type IssuePath = RepoPath & { issue_number: number };

export type AgentMediaPaths = {
  "/repos/{owner}/{repo}/pulls/{pull_number}": Item<
    PullPath,
    {
      get: {
        parameters: Params<PullPath>;
        requestBody?: never;
        responses: { 200: Json<AgentPull>; 404: Json<GithubError> };
      };
    }
  >;
  "/repos/{owner}/{repo}/issues/{issue_number}": Item<
    IssuePath,
    {
      get: {
        parameters: Params<IssuePath>;
        requestBody?: never;
        responses: { 200: Json<AgentIssue>; 404: Json<GithubError> };
      };
    }
  >;
};

export const agentMedia = createClient<AgentMediaPaths>({
  baseUrl: "/api/v3",
  headers: { Accept: AGENT_MEDIA_TYPE },
});
agentMedia.use(waitForMocks);

// ---- Errors ----

// ApiError keeps only the message. The Graftyard bodies carry fields the UI acts on
// (`errors`, `state`, `conflicts`, `oldest_cursor`), so this one keeps the body too.
export class GraftyardError extends ApiError {
  readonly body: Partial<GithubError & TaskConflict & LeaseConflict & FeedGone>;
  readonly retryAfter: number | undefined;
  constructor(response: Response, body: unknown) {
    super(response.status, body);
    this.body = body && typeof body === "object" ? body : {};
    const after = Number(response.headers.get("retry-after"));
    this.retryAfter = Number.isFinite(after) && after > 0 ? after : undefined;
  }
  // The field errors of a 422, by field name.
  fieldErrors(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const e of this.body.errors ?? [])
      out[e.field] =
        e.field === "session" && e.code === "invalid"
          ? "No live task by that id."
          : describeCode(e.code);
    return out;
  }
}

function describeCode(code: string) {
  if (code === "missing_field") return "This field is required.";
  if (code === "already_exists") return "This value is already taken.";
  return "This value is not valid.";
}

type Result<T> = { data?: T; error?: unknown; response: Response };

export async function call<T>(request: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await request;
  if (!response.ok || error !== undefined) throw new GraftyardError(response, error);
  return data as T;
}

// The next page's opaque `after`, from GitHub's Link header.
export function nextAfter(response: Response): string | undefined {
  const link = response.headers.get("link") ?? "";
  const next = link.split(",").find((part) => /rel="next"/.test(part));
  const url = next?.match(/<([^>]+)>/)?.[1];
  return url ? (new URL(url, location.origin).searchParams.get("after") ?? undefined) : undefined;
}
