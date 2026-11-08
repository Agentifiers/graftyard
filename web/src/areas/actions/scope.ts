import { api, unwrap } from "@/api/client";
import { queryOptions } from "@tanstack/react-query";

// Secrets, variables, and runners live at a repository or at the organization. One type
// names which, so each panel serves both and every call picks its path from it.
export type Scope = { kind: "repo"; owner: string; repo: string } | { kind: "org"; org: string };

// One secret or variable as the lists show it. `value` is null for a secret (GHES never
// returns one); `visibility` is null at a repository.
export type ConfigRow = {
  name: string;
  value: string | null;
  visibility: "all" | "private" | "selected" | null;
  created_at: string;
  updated_at: string;
};

export const scopeKey = (s: Scope) =>
  s.kind === "repo" ? ["repos", s.owner, s.repo, "actions"] : ["orgs", s.org, "actions"];

export const publicKeyQuery = (s: Scope) =>
  queryOptions({
    queryKey: [...scopeKey(s), "secrets", "public-key"],
    queryFn: ({ signal }) =>
      s.kind === "repo"
        ? unwrap(
            api.GET("/repos/{owner}/{repo}/actions/secrets/public-key", {
              params: { path: { owner: s.owner, repo: s.repo } },
              signal,
            }),
          )
        : unwrap(
            api.GET("/orgs/{org}/actions/secrets/public-key", {
              params: { path: { org: s.org } },
              signal,
            }),
          ),
    // A fresh key for each save: GHES may rotate it.
    staleTime: 0,
  });

export const secretsQuery = (s: Scope) =>
  queryOptions<ConfigRow[]>({
    queryKey: [...scopeKey(s), "secrets"],
    queryFn: async ({ signal }) => {
      if (s.kind === "repo") {
        const r = await unwrap(
          api.GET("/repos/{owner}/{repo}/actions/secrets", {
            params: { path: { owner: s.owner, repo: s.repo }, query: { per_page: 100 } },
            signal,
          }),
        );
        return r.secrets.map((x) => ({ ...x, value: null, visibility: null }));
      }
      const r = await unwrap(
        api.GET("/orgs/{org}/actions/secrets", {
          params: { path: { org: s.org }, query: { per_page: 100 } },
          signal,
        }),
      );
      return r.secrets.map(({ name, created_at, updated_at, visibility }) => ({
        name,
        created_at,
        updated_at,
        visibility,
        value: null,
      }));
    },
  });

export const variablesQuery = (s: Scope) =>
  queryOptions<ConfigRow[]>({
    queryKey: [...scopeKey(s), "variables"],
    queryFn: async ({ signal }) => {
      if (s.kind === "repo") {
        const r = await unwrap(
          api.GET("/repos/{owner}/{repo}/actions/variables", {
            params: { path: { owner: s.owner, repo: s.repo }, query: { per_page: 30 } },
            signal,
          }),
        );
        return r.variables.map((x) => ({ ...x, visibility: null }));
      }
      const r = await unwrap(
        api.GET("/orgs/{org}/actions/variables", {
          params: { path: { org: s.org }, query: { per_page: 30 } },
          signal,
        }),
      );
      return r.variables.map(({ name, value, created_at, updated_at, visibility }) => ({
        name,
        value,
        created_at,
        updated_at,
        visibility,
      }));
    },
  });

export const runnersQuery = (s: Scope) =>
  queryOptions({
    queryKey: [...scopeKey(s), "runners"],
    queryFn: ({ signal }) =>
      s.kind === "repo"
        ? unwrap(
            api.GET("/repos/{owner}/{repo}/actions/runners", {
              params: { path: { owner: s.owner, repo: s.repo }, query: { per_page: 100 } },
              signal,
            }),
          )
        : unwrap(
            api.GET("/orgs/{org}/actions/runners", {
              params: { path: { org: s.org }, query: { per_page: 100 } },
              signal,
            }),
          ),
    refetchInterval: 5000,
  });
