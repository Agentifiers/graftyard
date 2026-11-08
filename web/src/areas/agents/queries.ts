import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";
import { agentMedia, call, gy, type TaskState } from "./api";

// One factory per page region. Keys start with the REST path segments, under
// "graftyard" for the /api/graftyard/v1 paths.

type Repo = { owner: string; repo: string };

export type TaskFilters = {
  state?: TaskState;
  app_id?: number;
  requester?: string;
  issue_number?: number;
};

export const tasksKey = ({ owner, repo }: Repo) => ["graftyard", "repos", owner, repo, "tasks"];

export const tasksQuery = (r: Repo, filters: TaskFilters = {}) =>
  queryOptions({
    queryKey: [...tasksKey(r), filters],
    queryFn: ({ signal }) =>
      call(
        gy.GET("/repos/{owner}/{repo}/tasks", {
          params: { path: r, query: { ...filters, per_page: 100 } },
          signal,
        }),
      ),
    // Tasks move on their own; the list follows them.
    refetchInterval: 5_000,
  });

export const taskQuery = (r: Repo & { id: string }) =>
  queryOptions({
    queryKey: [...tasksKey(r), r.id],
    queryFn: ({ signal }) =>
      call(
        gy.GET("/repos/{owner}/{repo}/tasks/{id}", {
          params: { path: { owner: r.owner, repo: r.repo, id: r.id } },
          signal,
        }),
      ),
    refetchInterval: 5_000,
  });

export const logQuery = (r: Repo & { id: string }) =>
  queryOptions({
    queryKey: [...tasksKey(r), r.id, "log"],
    queryFn: ({ signal }) =>
      call(
        gy.GET("/repos/{owner}/{repo}/tasks/{id}/log", {
          params: { path: { owner: r.owner, repo: r.repo, id: r.id } },
          signal,
        }),
      ),
  });

export const leasesKey = ({ owner, repo }: Repo) => ["graftyard", "repos", owner, repo, "leases"];

export const leasesQuery = (r: Repo) =>
  queryOptions({
    queryKey: leasesKey(r),
    queryFn: ({ signal }) =>
      call(
        gy.GET("/repos/{owner}/{repo}/leases", {
          params: { path: r, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const policyQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["graftyard", "repos", r.owner, r.repo, "agents", "policy"],
    queryFn: ({ signal }) =>
      call(gy.GET("/repos/{owner}/{repo}/agents/policy", { params: { path: r }, signal })),
  });

export const orgAgentsQuery = (org: string) =>
  queryOptions({
    queryKey: ["graftyard", "orgs", org, "agents"],
    queryFn: ({ signal }) =>
      call(gy.GET("/orgs/{org}/agents", { params: { path: { org } }, signal })),
  });

export const grantsQuery = () =>
  queryOptions({
    queryKey: ["graftyard", "mcp", "grants"],
    queryFn: ({ signal }) => call(gy.GET("/mcp/grants", { signal })),
  });

export const agentAppsQuery = () =>
  queryOptions({
    queryKey: ["graftyard", "agents", "apps"],
    queryFn: ({ signal }) => call(gy.GET("/agents/apps", { signal })),
    staleTime: 5 * 60_000,
  });

// The opt-in trust view of a pull request. A separate key from the pulls area's
// ["repos", owner, repo, "pulls", n], because the reply differs under this media type.
export const agentPullQuery = (r: Repo & { number: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "pulls", r.number, "agent+json"],
    queryFn: ({ signal }) =>
      call(
        agentMedia.GET("/repos/{owner}/{repo}/pulls/{pull_number}", {
          params: { path: { owner: r.owner, repo: r.repo, pull_number: r.number } },
          signal,
        }),
      ),
  });

// Lease holders and feed events name users by id.
export const accountQuery = (accountId: number) =>
  queryOptions({
    queryKey: ["user", accountId],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/user/{account_id}", { params: { path: { account_id: accountId } }, signal }),
      ),
    staleTime: 5 * 60_000,
  });
