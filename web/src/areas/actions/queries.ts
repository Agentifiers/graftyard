import { queryOptions } from "@tanstack/react-query";
import { api, ApiError, unwrap, type Schemas } from "@/api/client";
import type { StatusFilter } from "./status";
import { ui } from "./ui-api";

// One factory per page region. Keys are the REST path segments.

type Repo = { owner: string; repo: string };

export type RunFilters = {
  workflow?: number;
  branch?: string;
  event?: string;
  status?: StatusFilter;
  actor?: string;
  page?: number;
};

export const RUNS_PER_PAGE = 15;

// While something is still running, poll; once everything is final, stop.
const live = (busy: boolean) => (busy ? 2000 : false);
const runBusy = (r: { status: string | null }) => r.status !== "completed";

export const workflowsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "workflows"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/workflows", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const runsQuery = ({ owner, repo, ...f }: Repo & RunFilters) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", f],
    queryFn: ({ signal }) => {
      const query = {
        branch: f.branch,
        event: f.event,
        status: f.status,
        actor: f.actor,
        per_page: RUNS_PER_PAGE,
        page: f.page ?? 1,
      };
      return f.workflow
        ? unwrap(
            api.GET("/repos/{owner}/{repo}/actions/workflows/{workflow_id}/runs", {
              params: { path: { owner, repo, workflow_id: f.workflow }, query },
              signal,
            }),
          )
        : unwrap(
            api.GET("/repos/{owner}/{repo}/actions/runs", {
              params: { path: { owner, repo }, query },
              signal,
            }),
          );
    },
    refetchInterval: (q) => live(q.state.data?.workflow_runs.some(runBusy) ?? false),
  });

export const workflowSourceQuery = ({ owner, repo, workflowId }: Repo & { workflowId: number }) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", owner, repo, "actions", "workflows", workflowId, "source"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/actions/workflows/{workflow_id}/source", {
          params: { path: { owner, repo, workflow_id: String(workflowId) } },
          signal,
        }),
      ),
  });

type RunRef = Repo & { runId: number };

export const runQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", runId],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runs/{run_id}", {
          params: { path: { owner, repo, run_id: runId } },
          signal,
        }),
      ),
    refetchInterval: (q) => live(q.state.data ? runBusy(q.state.data) : false),
  });

export const jobsQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", runId, "jobs"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runs/{run_id}/jobs", {
          params: { path: { owner, repo, run_id: runId }, query: { per_page: 100 } },
          signal,
        }),
      ),
    refetchInterval: (q) => live(q.state.data?.jobs.some(runBusy) ?? false),
  });

export const runGraphQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", owner, repo, "actions", "runs", runId, "graph"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/actions/runs/{run_id}/graph", {
          params: { path: { owner, repo, run_id: runId } },
          signal,
        }),
      ),
  });

export const artifactsQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", runId, "artifacts"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runs/{run_id}/artifacts", {
          params: { path: { owner, repo, run_id: runId }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const pendingDeploymentsQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", runId, "pending_deployments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runs/{run_id}/pending_deployments", {
          params: { path: { owner, repo, run_id: runId } },
          signal,
        }),
      ),
  });

export const approvalsQuery = ({ owner, repo, runId }: RunRef) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runs", runId, "approvals"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runs/{run_id}/approvals", {
          params: { path: { owner, repo, run_id: runId } },
          signal,
        }),
      ),
  });

// The annotations of every job in a run, read from each job's check run. A re-run keeps
// the check run ids, so `stamp` (each job's attempt and state) is in the key to refetch.
export const annotationsQuery = ({
  owner,
  repo,
  checkRunIds,
  stamp,
}: Repo & { checkRunIds: number[]; stamp: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "check-runs", checkRunIds, "annotations", stamp],
    queryFn: ({ signal }) =>
      Promise.all(
        checkRunIds.map((id) =>
          unwrap(
            api.GET("/repos/{owner}/{repo}/check-runs/{check_run_id}/annotations", {
              params: { path: { owner, repo, check_run_id: id }, query: { per_page: 100 } },
              signal,
            }),
          ).then((list) => list.map((a) => ({ ...a, check_run_id: id }))),
        ),
      ).then((lists) => lists.flat()),
  });

export const jobQuery = ({ owner, repo, jobId }: Repo & { jobId: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "jobs", jobId],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/jobs/{job_id}", {
          params: { path: { owner, repo, job_id: jobId } },
          signal,
        }),
      ),
    refetchInterval: (q) => live(q.state.data ? runBusy(q.state.data) : false),
  });

// GHES answers 302 to a signed blob; fetch follows it, and the body is the plain log.
export const jobLogQuery = ({ owner, repo, jobId }: Repo & { jobId: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "jobs", jobId, "logs"],
    queryFn: async ({ signal }) => {
      const { data, error, response } = await api.GET(
        "/repos/{owner}/{repo}/actions/jobs/{job_id}/logs",
        {
          params: { path: { owner, repo, job_id: jobId } },
          parseAs: "text",
          signal,
        },
      );
      if (!response.ok || error !== undefined) throw new ApiError(response.status, error);
      return parseLog(data ?? "");
    },
  });

// A saved log line is `<RFC 3339 timestamp> <text>`.
export type SavedLine = { at: string; text: string };

export function parseLog(text: string): SavedLine[] {
  if (!text) return [];
  return text.split("\n").map((line) => {
    const space = line.indexOf(" ");
    return space > 0
      ? { at: line.slice(0, space), text: line.slice(space + 1) }
      : { at: "", text: line };
  });
}

// ---- Settings ----

export const actionsPermissionsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "permissions"],
    queryFn: async ({ signal }) => {
      const p = { params: { path: { owner, repo } }, signal };
      const [permissions, selected, workflow, fork, retention] = await Promise.all([
        unwrap(api.GET("/repos/{owner}/{repo}/actions/permissions", p)),
        unwrap(api.GET("/repos/{owner}/{repo}/actions/permissions/selected-actions", p)),
        unwrap(api.GET("/repos/{owner}/{repo}/actions/permissions/workflow", p)),
        unwrap(
          api.GET("/repos/{owner}/{repo}/actions/permissions/fork-pr-contributor-approval", p),
        ),
        unwrap(api.GET("/repos/{owner}/{repo}/actions/permissions/artifact-and-log-retention", p)),
      ]);
      return { permissions, selected, workflow, fork, retention };
    },
  });

export type ActionsPermissions = {
  permissions: Schemas["actions-repository-permissions"];
  selected: Schemas["selected-actions"];
  workflow: Schemas["actions-get-default-workflow-permissions"];
  fork: Schemas["actions-fork-pr-contributor-approval"];
  retention: Schemas["actions-artifact-and-log-retention-response"];
};

export const repoOrgSecretsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "organization-secrets"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/organization-secrets", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const repoOrgVariablesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "organization-variables"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/organization-variables", {
          params: { path: { owner, repo }, query: { per_page: 30 } },
          signal,
        }),
      ),
  });

// The environments list and each one's secrets (the releases area owns both routes).
export const environmentSecretsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "environments", "secrets"],
    queryFn: async ({ signal }) => {
      const envs = await unwrap(
        api.GET("/repos/{owner}/{repo}/environments", {
          params: { path: { owner, repo } },
          signal,
        }),
      );
      return Promise.all(
        (envs.environments ?? []).map(async (env) => ({
          env,
          secrets: (
            await unwrap(
              api.GET("/repos/{owner}/{repo}/environments/{environment_name}/secrets", {
                params: { path: { owner, repo, environment_name: env.name } },
                signal,
              }),
            )
          ).secrets,
        })),
      );
    },
  });

export const environmentsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "environments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/environments", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

export const branchesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "branches"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/branches", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const runnerDownloadsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "runners", "downloads"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/runners/downloads", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
    staleTime: Infinity,
  });

export const cachesQuery = ({
  owner,
  repo,
  ref,
  sort,
  direction,
}: Repo & {
  ref?: string;
  sort: "last_accessed_at" | "created_at" | "size_in_bytes";
  direction: "asc" | "desc";
}) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "caches", { ref, sort, direction }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/actions/caches", {
          params: { path: { owner, repo }, query: { ref, sort, direction, per_page: 100 } },
          signal,
        }),
      ),
  });

export const cacheUsageQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "actions", "cache", "usage"],
    queryFn: async ({ signal }) => {
      const p = { params: { path: { owner, repo } }, signal };
      const [usage, policy] = await Promise.all([
        unwrap(api.GET("/repos/{owner}/{repo}/actions/cache/usage", p)),
        unwrap(api.GET("/repos/{owner}/{repo}/actions/cache/usage-policy", p)),
      ]);
      return { usage, policy };
    },
  });

// ---- Organization ----

export const orgSecretReposQuery = (org: string, name: string) =>
  queryOptions({
    queryKey: ["orgs", org, "actions", "secrets", name, "repositories"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/actions/secrets/{secret_name}/repositories", {
          params: { path: { org, secret_name: name } },
          signal,
        }),
      ),
  });

export const orgVariableReposQuery = (org: string, name: string) =>
  queryOptions({
    queryKey: ["orgs", org, "actions", "variables", name, "repositories"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/actions/variables/{name}/repositories", {
          params: { path: { org, name } },
          signal,
        }),
      ),
  });

export const runnerGroupsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "actions", "runner-groups"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/actions/runner-groups", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const runnerGroupReposQuery = (org: string, groupId: number) =>
  queryOptions({
    queryKey: ["orgs", org, "actions", "runner-groups", groupId, "repositories"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/actions/runner-groups/{runner_group_id}/repositories", {
          params: { path: { org, runner_group_id: groupId } },
          signal,
        }),
      ),
  });

export const orgReposQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "repos"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/repos", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });
