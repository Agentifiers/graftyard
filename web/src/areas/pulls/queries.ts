import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";

type Repo = { owner: string; repo: string };

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
