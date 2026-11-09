import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";

// The viewer, repositories, and organizations: the reads the shared layouts use. One
// factory per page region; each area keeps its own in src/areas/<area>/queries.ts. Keys
// start with the REST path segments so the edge can later seed the first route's cache.

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
