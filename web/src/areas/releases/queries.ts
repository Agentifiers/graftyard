import { queryOptions } from "@tanstack/react-query";
import { api, ApiError, unwrap, type Schemas } from "@/api/client";
import { ui } from "./api";

// One factory per page region. Keys start with the REST path segments, so a write
// invalidates every read under the path it changed.

type Repo = { owner: string; repo: string };
type Env = Repo & { name: string };

export const RELEASES_PER_PAGE = 4;

// A 404 from /releases/latest means the repository has no full release yet.
async function orNull<T>(call: Promise<T>): Promise<T | null> {
  try {
    return await call;
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

export const releasesKey = ({ owner, repo }: Repo) => ["repos", owner, repo, "releases"] as const;

// One page of releases and whether a next page exists, read from GHES's Link header.
export const releasesPageQuery = ({ owner, repo, page }: Repo & { page: number }) =>
  queryOptions({
    queryKey: [...releasesKey({ owner, repo }), { page, per_page: RELEASES_PER_PAGE }],
    queryFn: async ({ signal }) => {
      const { data, error, response } = await api.GET("/repos/{owner}/{repo}/releases", {
        params: { path: { owner, repo }, query: { page, per_page: RELEASES_PER_PAGE } },
        signal,
      });
      if (!response.ok || error !== undefined) throw new ApiError(response.status, error);
      const link = response.headers.get("link") ?? "";
      return { releases: data, hasNext: link.includes('rel="next"') };
    },
  });

// Every release, for counts, the edit page's lookup by tag, and the tag picker.
export const allReleasesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: [...releasesKey({ owner, repo }), { per_page: 100 }],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/releases", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const latestReleaseQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: [...releasesKey({ owner, repo }), "latest"],
    queryFn: ({ signal }) =>
      orNull(
        unwrap(
          api.GET("/repos/{owner}/{repo}/releases/latest", {
            params: { path: { owner, repo } },
            signal,
          }),
        ),
      ),
  });

export const releaseByTagQuery = ({ owner, repo, tag }: Repo & { tag: string }) =>
  queryOptions({
    queryKey: [...releasesKey({ owner, repo }), "tags", tag],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/releases/tags/{tag}", {
          params: { path: { owner, repo, tag } },
          signal,
        }),
      ),
  });

export const reactionsQuery = ({ owner, repo, id }: Repo & { id: number }) =>
  queryOptions({
    queryKey: [...releasesKey({ owner, repo }), id, "reactions"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/releases/{release_id}/reactions", {
          params: { path: { owner, repo, release_id: id }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// Owned by the code area; the same keys its queries use.
export const tagsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "tags"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/tags", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
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

// Owned by repo-admin; the reviewer picker reads them.
export const collaboratorsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "collaborators"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/collaborators", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const repoTeamsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "teams"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/teams", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

// ---- Deployments ----

export const deploymentsKey = ({ owner, repo }: Repo) =>
  ["repos", owner, repo, "deployments"] as const;

export type DeploymentRow = {
  deployment: Schemas["deployment"];
  // Newest first, as GHES lists them.
  statuses: Schemas["deployment-status"][];
};

const FINAL = new Set(["success", "failure", "error", "inactive"]);
export const isFinal = (row: DeploymentRow) => FINAL.has(row.statuses[0]?.state ?? "");
// A deployment held for review stays queued until someone reviews it, so it is not polled.
const isHeld = (row: DeploymentRow) =>
  row.statuses[0]?.state === "queued" && row.statuses[0].description === "Waiting for review";

// Each deployment with its statuses. Polls while any deployment is still moving.
export const deploymentHistoryQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: [...deploymentsKey({ owner, repo }), "history"],
    queryFn: async ({ signal }): Promise<DeploymentRow[]> => {
      const deployments = await unwrap(
        api.GET("/repos/{owner}/{repo}/deployments", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      );
      return Promise.all(
        deployments.map(async (deployment) => ({
          deployment,
          statuses: await unwrap(
            api.GET("/repos/{owner}/{repo}/deployments/{deployment_id}/statuses", {
              params: {
                path: { owner, repo, deployment_id: deployment.id },
                query: { per_page: 100 },
              },
              signal,
            }),
          ),
        })),
      );
    },
    refetchInterval: (q) =>
      q.state.data?.some((row) => !isFinal(row) && !isHeld(row)) ? 1500 : false,
  });

export const pendingReviewsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["_t", "ui", ...deploymentsKey({ owner, repo }), "pending"],
    queryFn: ({ signal }) =>
      unwrap(
        ui.GET("/repos/{owner}/{repo}/deployments/pending", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

// ---- Environments ----

export const environmentsKey = ({ owner, repo }: Repo) =>
  ["repos", owner, repo, "environments"] as const;

export const environmentsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), "list"],
    queryFn: async ({ signal }) => {
      const data = await unwrap(
        api.GET("/repos/{owner}/{repo}/environments", {
          params: { path: { owner, repo }, query: { per_page: 100 } },
          signal,
        }),
      );
      return data.environments ?? [];
    },
  });

export const environmentQuery = ({ owner, repo, name }: Env) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), name],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/environments/{environment_name}", {
          params: { path: { owner, repo, environment_name: name } },
          signal,
        }),
      ),
  });

export const branchPoliciesQuery = ({ owner, repo, name }: Env) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), name, "deployment-branch-policies"],
    queryFn: async ({ signal }) =>
      (
        await unwrap(
          api.GET(
            "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies",
            {
              params: { path: { owner, repo, environment_name: name }, query: { per_page: 100 } },
              signal,
            },
          ),
        )
      ).branch_policies,
  });

export const envSecretsQuery = ({ owner, repo, name }: Env) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), name, "secrets"],
    queryFn: async ({ signal }) =>
      (
        await unwrap(
          api.GET("/repos/{owner}/{repo}/environments/{environment_name}/secrets", {
            params: { path: { owner, repo, environment_name: name }, query: { per_page: 100 } },
            signal,
          }),
        )
      ).secrets,
  });

export const envVariablesQuery = ({ owner, repo, name }: Env) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), name, "variables"],
    queryFn: async ({ signal }) =>
      (
        await unwrap(
          api.GET("/repos/{owner}/{repo}/environments/{environment_name}/variables", {
            params: { path: { owner, repo, environment_name: name }, query: { per_page: 30 } },
            signal,
          }),
        )
      ).variables,
  });

export const envPublicKeyQuery = ({ owner, repo, name }: Env) =>
  queryOptions({
    queryKey: [...environmentsKey({ owner, repo }), name, "secrets", "public-key"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/environments/{environment_name}/secrets/public-key", {
          params: { path: { owner, repo, environment_name: name } },
          signal,
        }),
      ),
  });
