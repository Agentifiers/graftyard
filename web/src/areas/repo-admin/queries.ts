import { queryOptions } from "@tanstack/react-query";
import { api } from "@/api/client";
import { call } from "./client";
import { ui } from "./private-api";

// One factory per page region. Keys start with the REST path segments.

type Repo = { owner: string; repo: string };
const path = ({ owner, repo }: Repo) => ({ path: { owner, repo } });

export const topicsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "topics"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/topics", { params: path(r), signal })),
  });

export const adminSettingsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", r.owner, r.repo, "admin-settings"],
    queryFn: ({ signal }) =>
      call(ui.GET("/repos/{owner}/{repo}/admin-settings", { params: path(r), signal })),
  });

// The branch names, read from the branches list the code area owns.
export const branchesQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "branches"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/branches", {
          params: { ...path(r), query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const collaboratorsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "collaborators"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/collaborators", {
          params: { ...path(r), query: { affiliation: "direct", per_page: 100 } },
          signal,
        }),
      ),
  });

export const invitationsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "invitations"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/invitations", { params: path(r), signal })),
  });

export const repoTeamsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "teams"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/teams", { params: path(r), signal })),
  });

// Users for the invite autocomplete, through GET /users, which the orgs and admin areas
// also read.
export const usersQuery = () =>
  queryOptions({
    queryKey: ["users"],
    queryFn: ({ signal }) =>
      call(api.GET("/users", { params: { query: { per_page: 100 } }, signal })),
    staleTime: 5 * 60_000,
  });

export const protectionRulesQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", r.owner, r.repo, "protection-rules"],
    queryFn: ({ signal }) =>
      call(ui.GET("/repos/{owner}/{repo}/protection-rules", { params: path(r), signal })),
  });

export const protectionQuery = (r: Repo & { branch: string }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "branches", r.branch, "protection"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/branches/{branch}/protection", {
          params: { path: { owner: r.owner, repo: r.repo, branch: r.branch } },
          signal,
        }),
      ),
  });

export const checkNamesQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", r.owner, r.repo, "check-names"],
    queryFn: ({ signal }) =>
      call(ui.GET("/repos/{owner}/{repo}/check-names", { params: path(r), signal })),
  });

export const rulesetsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "rulesets"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/rulesets", { params: path(r), signal })),
  });

export const rulesetQuery = (r: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "rulesets", r.id],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/rulesets/{ruleset_id}", {
          params: { path: { owner: r.owner, repo: r.repo, ruleset_id: r.id } },
          signal,
        }),
      ),
  });

export const rulesetHistoryQuery = (r: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "rulesets", r.id, "history"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/rulesets/{ruleset_id}/history", {
          params: { path: { owner: r.owner, repo: r.repo, ruleset_id: r.id } },
          signal,
        }),
      ),
  });

export type SuiteFilters = {
  time_period: "hour" | "day" | "week" | "month";
  rule_suite_result: "pass" | "fail" | "bypass" | "all";
  actor_name?: string;
};

export const ruleSuitesQuery = (r: Repo & SuiteFilters) =>
  queryOptions({
    queryKey: [
      "repos",
      r.owner,
      r.repo,
      "rulesets",
      "rule-suites",
      r.time_period,
      r.rule_suite_result,
      r.actor_name ?? "",
    ],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/rulesets/rule-suites", {
          params: {
            ...path(r),
            query: {
              time_period: r.time_period,
              rule_suite_result: r.rule_suite_result,
              actor_name: r.actor_name || undefined,
              per_page: 100,
            },
          },
          signal,
        }),
      ),
  });

export const ruleSuiteQuery = (r: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "rulesets", "rule-suites", r.id],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/rulesets/rule-suites/{rule_suite_id}", {
          params: { path: { owner: r.owner, repo: r.repo, rule_suite_id: r.id } },
          signal,
        }),
      ),
  });

export const autolinksQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "autolinks"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/autolinks", { params: path(r), signal })),
  });

export const hooksQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "hooks"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/hooks", { params: path(r), signal })),
  });

export const hookQuery = (r: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "hooks", r.id],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/hooks/{hook_id}", {
          params: { path: { owner: r.owner, repo: r.repo, hook_id: r.id } },
          signal,
        }),
      ),
  });

export const deliveriesQuery = (r: Repo & { id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "hooks", r.id, "deliveries"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries", {
          params: {
            path: { owner: r.owner, repo: r.repo, hook_id: r.id },
            query: { per_page: 50 },
          },
          signal,
        }),
      ),
  });

export const deliveryQuery = (r: Repo & { hook: number; id: number }) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "hooks", r.hook, "deliveries", r.id],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}", {
          params: { path: { owner: r.owner, repo: r.repo, hook_id: r.hook, delivery_id: r.id } },
          signal,
        }),
      ),
    staleTime: Infinity,
  });

export const deployKeysQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "keys"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/keys", { params: path(r), signal })),
  });

export const preReceiveQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "pre-receive-hooks"],
    queryFn: ({ signal }) =>
      call(api.GET("/repos/{owner}/{repo}/pre-receive-hooks", { params: path(r), signal })),
  });

export const installationsQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["_t", "ui", "repos", r.owner, r.repo, "installations"],
    queryFn: ({ signal }) =>
      call(ui.GET("/repos/{owner}/{repo}/installations", { params: path(r), signal })),
  });

export const gitignoreTemplatesQuery = () =>
  queryOptions({
    queryKey: ["gitignore", "templates"],
    queryFn: ({ signal }) => call(api.GET("/gitignore/templates", { signal })),
    staleTime: Infinity,
  });

export const licensesQuery = () =>
  queryOptions({
    queryKey: ["licenses"],
    queryFn: ({ signal }) =>
      call(api.GET("/licenses", { params: { query: { per_page: 100 } }, signal })),
    staleTime: Infinity,
  });

// Whether owner/name exists: GET /repos/{owner}/{repo} answers 404 when the name is free.
export const nameTakenQuery = (owner: string, name: string) =>
  queryOptions({
    queryKey: ["repos", owner, name, "exists"],
    queryFn: async ({ signal }) => {
      const { response } = await api.GET("/repos/{owner}/{repo}", {
        params: { path: { owner, repo: name } },
        signal,
      });
      if (response.status === 404) return false;
      if (response.ok) return true;
      throw new Error(`HTTP ${response.status}`);
    },
    staleTime: 0,
    retry: false,
  });

export const forksQuery = (r: Repo) =>
  queryOptions({
    queryKey: ["repos", r.owner, r.repo, "forks"],
    queryFn: ({ signal }) =>
      call(
        api.GET("/repos/{owner}/{repo}/forks", {
          params: { ...path(r), query: { sort: "newest", per_page: 30 } },
          signal,
        }),
      ),
  });
