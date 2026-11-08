import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";

type Repo = { owner: string; repo: string };

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
