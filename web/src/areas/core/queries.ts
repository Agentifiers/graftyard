import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";

// One factory per page region. Keys start with the REST path segments so the edge can
// later seed the cache for the first route from inlined data.

type Repo = { owner: string; repo: string };

export const userQuery = () =>
  queryOptions({
    queryKey: ["user"],
    queryFn: ({ signal }) => unwrap(api.GET("/user", { signal })),
  });

export const userReposQuery = () =>
  queryOptions({
    queryKey: ["user", "repos"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/user/repos", { params: { query: { sort: "full_name", per_page: 100 } }, signal }),
      ),
  });

export const repoQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/repos/{owner}/{repo}", { params: { path: { owner, repo } }, signal })),
  });

// The GHES issues list includes pull requests; the UI filters them out.
export const issuesQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues", {
          params: { path: { owner, repo }, query: { state: "all", per_page: 100 } },
          signal,
        }),
      ),
  });

export const issueQuery = ({ owner, repo, number }: Repo & { number: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}", {
          params: { path: { owner, repo, issue_number: number } },
          signal,
        }),
      ),
  });

export const commentsQuery = ({ owner, repo, number }: Repo & { number: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "issues", number, "comments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/issues/{issue_number}/comments", {
          params: { path: { owner, repo, issue_number: number }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const pullsQuery = ({ owner, repo }: Repo) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pulls"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls", {
          params: { path: { owner, repo }, query: { state: "all", per_page: 100 } },
          signal,
        }),
      ),
  });

export const pullQuery = ({ owner, repo, number }: Repo & { number: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pulls", number],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}", {
          params: { path: { owner, repo, pull_number: number } },
          signal,
        }),
      ),
  });

export const pullFilesQuery = ({ owner, repo, number }: Repo & { number: number }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pulls", number, "files"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pulls/{pull_number}/files", {
          params: { path: { owner, repo, pull_number: number }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const checkRunsQuery = ({ owner, repo, ref }: Repo & { ref: string }) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "commits", ref, "check-runs"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/commits/{ref}/check-runs", {
          params: { path: { owner, repo, ref } },
          signal,
        }),
      ),
  });

export const orgQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org],
    queryFn: ({ signal }) => unwrap(api.GET("/orgs/{org}", { params: { path: { org } }, signal })),
  });

export const userOrgsQuery = () =>
  queryOptions({
    queryKey: ["user", "orgs"],
    queryFn: ({ signal }) => unwrap(api.GET("/user/orgs", { signal })),
  });
